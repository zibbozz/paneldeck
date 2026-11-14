//! Quick-Actions (Schnellaktionen) und die Logik des Quick-Menüs (`.` im Ticket): reine Datenschicht ohne Oberfläche.
//!
//! * `schema` – SQL der Tabelle `quick_actions`.
//! * `params` – Aktionsarten (`create_ticket`, `change_status`, `start_session`) und ihre Parameter als JSON mit Format-Version.
//! * `actions` – Anlegen/Ändern/Löschen/Ordnen/Duplizieren, Prüfung gegen die Konfiguration, Aufräumen verwaister Verweise.
//! * `resolve` – Auflösung der Defaults einer `create_ticket`-Aktion (kleine, sicher begrenzte Platzhalter-Ersetzung).
//! * `tpl` – Rundreise durch den Typ-Vorlagen-Export/-Import (`planning::template`), Verweise per Vorlagen-Nummer.
//! * `input`, `matcher`, `menu` – Quick-Menü: Einträge, tolerante Suche, Eingabeformen und Prüfung der Eingabe.
//!
//! Fehler: [`QuickError`] (zweisprachig über `message(lang)`), Fehler der übrigen Konfiguration reichen als
//! [`QuickError::Plan`] durch. Mehrstufige Änderungen laufen atomar (SAVEPOINT) und lassen sich verschachteln.

pub mod actions;
pub mod input;
pub mod matcher;
pub mod menu;
pub mod params;
pub mod resolve;
pub mod schema;
pub mod tpl;

use super::{PlanError, TemplateProblem};
use crate::state::Lang;
use rusqlite::Connection;

/// Wie ein Eintrag im Fehlertext benannt wird (Titel, Feld mit Namen/Schlüssel oder nur Kennung).
#[derive(Debug, Clone, PartialEq)]
pub enum FieldLabel {
    Title,
    Field { key: Option<String>, name: Option<String> },
    Id(i64),
}

impl FieldLabel {
    pub fn display(&self, lang: Lang) -> String {
        match self {
            FieldLabel::Title => lang.t("Titel", "Title").to_string(),
            FieldLabel::Field { key, name } => super::names::display_name(key.as_deref(), name.as_deref(), lang),
            FieldLabel::Id(id) => format!("#{id}"),
        }
    }
}

/// Was an einem Feldeintrag (Standardwert, zu setzender Wert) nicht stimmt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldProblem {
    /// Das Feld gehört nicht zum Ziel-Typ (bzw. zum Typ der Aktion).
    NotInType,
    /// Das Feld kommt zweimal vor.
    Duplicate,
    /// Der Wert passt nicht zum Feld.
    Value,
    /// Für diesen Feldtyp gibt es keinen Standardwert (Terminal).
    NoDefault,
}

#[derive(Debug, Clone, PartialEq)]
pub enum QuickError {
    /// Fehler der übrigen Konfiguration (Namen, nicht gefunden, Workflow, Datenbank …).
    Plan(PlanError),
    /// Ziel-Typ fehlt, gehört zu einem anderen Projekt oder ist ein System-Typ.
    TargetType,
    Field { field: FieldLabel, problem: FieldProblem },
    /// Verknüpfungstyp fehlt oder zeigt nicht auf Tickets.
    Relation,
    /// Shell-Kennung leer, zu lang oder mit Steuerzeichen.
    Shell,
    /// „Ticketinhalt als Prompt“ ohne Claude würde Text als Befehl an eine Shell senden.
    PromptNeedsClaude,
    /// Unbekannter Platzhalter `{{name}}`.
    Placeholder(String),
    /// Zu viele Einträge (Aktionen je Typ, Standardwerte je Aktion).
    TooMany { max: usize },
    /// Text zu lang.
    TooLong { max: usize },
    /// Parameter nicht lesbar (JSON).
    Params(String),
    /// Parameter mit neuerer Format-Version als bekannt.
    ParamsVersion { found: u32, supported: u32 },
    /// Die Aktion hat die falsche Art für diese Funktion.
    WrongKind,
    /// Die Aktion ist als ungültig markiert und lässt sich nicht ausführen.
    ActionInvalid,
}

pub type QRes<T> = Result<T, QuickError>;

impl From<PlanError> for QuickError {
    fn from(e: PlanError) -> Self {
        QuickError::Plan(e)
    }
}

impl From<rusqlite::Error> for QuickError {
    fn from(e: rusqlite::Error) -> Self {
        QuickError::Plan(PlanError::from(e))
    }
}

/// Für den Vorlagen-Import/-Export, der in `PlanError` rechnet.
impl From<QuickError> for PlanError {
    fn from(e: QuickError) -> Self {
        match e {
            QuickError::Plan(p) => p,
            other => PlanError::Template(TemplateProblem::Invalid(format!("quick action: {}", other.message(Lang::En)))),
        }
    }
}

impl QuickError {
    pub fn message(&self, lang: Lang) -> String {
        let s = |de: &'static str, en: &'static str| lang.t(de, en).to_string();
        match self {
            QuickError::Plan(p) => p.message(lang),
            QuickError::TargetType => s(
                "Der Ziel-Typ gehört nicht zu diesem Projekt oder ist ein Systemtyp.",
                "The target type does not belong to this project or is a system type.",
            ),
            QuickError::Field { field, problem } => {
                let q = lang.q(&field.display(lang));
                match (problem, lang) {
                    (FieldProblem::NotInType, Lang::De) => format!("Das Feld {q} gehört nicht zum Typ."),
                    (FieldProblem::NotInType, Lang::En) => format!("The field {q} does not belong to the type."),
                    (FieldProblem::Duplicate, Lang::De) => format!("Das Feld {q} kommt mehrfach vor."),
                    (FieldProblem::Duplicate, Lang::En) => format!("The field {q} appears more than once."),
                    (FieldProblem::Value, Lang::De) => format!("Der Wert für {q} passt nicht zum Feld."),
                    (FieldProblem::Value, Lang::En) => format!("The value for {q} does not fit the field."),
                    (FieldProblem::NoDefault, Lang::De) => format!("Für das Feld {q} gibt es keinen Standardwert."),
                    (FieldProblem::NoDefault, Lang::En) => format!("The field {q} cannot have a default value."),
                }
            }
            QuickError::Relation => s("Der Verknüpfungstyp fehlt oder verbindet keine Tickets.", "The link type is missing or does not connect tickets."),
            QuickError::Shell => s("Die Shell-Kennung ist ungültig.", "The shell id is not valid."),
            QuickError::PromptNeedsClaude => s(
                "Der Ticketinhalt als Prompt ist nur mit Claude möglich (sonst würde er als Befehl in die Shell geschrieben).",
                "Using the ticket as a prompt needs Claude (otherwise it would be typed into the shell as a command).",
            ),
            QuickError::Placeholder(p) => match lang {
                Lang::De => format!("Unbekannter Platzhalter {}.", lang.q(p)),
                Lang::En => format!("Unknown placeholder {}.", lang.q(p)),
            },
            QuickError::TooMany { max } => match lang {
                Lang::De => format!("Zu viele Einträge (höchstens {max})."),
                Lang::En => format!("Too many entries (at most {max})."),
            },
            QuickError::TooLong { max } => match lang {
                Lang::De => format!("Der Text ist zu lang (höchstens {max} Zeichen)."),
                Lang::En => format!("The text is too long (at most {max} characters)."),
            },
            QuickError::Params(e) => format!("{}: {e}", lang.t("Die Parameter der Aktion sind nicht lesbar", "The action's parameters cannot be read")),
            QuickError::ParamsVersion { found, supported } => format!(
                "{} {found}, {} {supported}.",
                lang.t("Die Aktion hat die Format-Version", "The action has format version"),
                lang.t("bekannt ist nur bis", "but only up to")
            ),
            QuickError::WrongKind => s("Diese Aktion hat eine andere Art.", "This action is of a different kind."),
            QuickError::ActionInvalid => s(
                "Die Aktion ist ungültig (ein Verweis existiert nicht mehr). Bitte im Typ-Editor korrigieren.",
                "The action is invalid (a reference no longer exists). Please fix it in the type editor.",
            ),
        }
    }
}

/// Führt `f` atomar aus (SAVEPOINT): bei einem Fehler wird alles zurückgenommen. Verschachtelbar.
pub(crate) fn atomic<T>(conn: &Connection, f: impl FnOnce() -> QRes<T>) -> QRes<T> {
    conn.execute_batch("SAVEPOINT quick_op")?;
    match f() {
        Ok(v) => {
            conn.execute_batch("RELEASE quick_op")?;
            Ok(v)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK TO quick_op; RELEASE quick_op");
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_error_has_both_languages() {
        let all = vec![
            QuickError::Plan(PlanError::NotFound),
            QuickError::TargetType,
            QuickError::Field { field: FieldLabel::Title, problem: FieldProblem::NotInType },
            QuickError::Field { field: FieldLabel::Id(4), problem: FieldProblem::Duplicate },
            QuickError::Field { field: FieldLabel::Field { key: Some("priority".into()), name: None }, problem: FieldProblem::Value },
            QuickError::Field { field: FieldLabel::Field { key: None, name: Some("X".into()) }, problem: FieldProblem::NoDefault },
            QuickError::Relation,
            QuickError::Shell,
            QuickError::PromptNeedsClaude,
            QuickError::Placeholder("source.foo".into()),
            QuickError::TooMany { max: 50 },
            QuickError::TooLong { max: 100 },
            QuickError::Params("x".into()),
            QuickError::ParamsVersion { found: 3, supported: 1 },
            QuickError::WrongKind,
            QuickError::ActionInvalid,
        ];
        for e in all {
            let (de, en) = (e.message(Lang::De), e.message(Lang::En));
            assert!(!de.is_empty() && !en.is_empty(), "{e:?}");
            assert_ne!(de, en, "{e:?}: deutsch und englisch sollten sich unterscheiden");
        }
        let e = QuickError::Field { field: FieldLabel::Field { key: Some("priority".into()), name: None }, problem: FieldProblem::Value };
        assert!(e.message(Lang::De).contains("Priorität") && e.message(Lang::En).contains("Priority"));
        assert!(QuickError::TooMany { max: 50 }.message(Lang::En).contains("50"));
    }

    #[test]
    fn plan_errors_pass_through_both_ways() {
        let q: QuickError = PlanError::NameTaken.into();
        assert_eq!(q, QuickError::Plan(PlanError::NameTaken));
        assert_eq!(PlanError::from(q), PlanError::NameTaken);
        assert!(matches!(PlanError::from(QuickError::Relation), PlanError::Template(TemplateProblem::Invalid(_))));
    }
}
