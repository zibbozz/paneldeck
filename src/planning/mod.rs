//! Konfigurations-Schicht der Projekte: Projekte, Status, Typen mit Hierarchie und Workflows, Felder, Verknüpfungstypen,
//! Arten der Arbeit, Vorlagen-Export/-Import. Reine Datenlogik auf `&Connection` ohne Oberfläche (Stil wie `tasks.rs`).
//!
//! * Übersetzbare Standardnamen: `key` gesetzt und `name` leer → Anzeige aus `names.rs`; Umbenennen setzt `name`.
//! * Löschen mit Integritätsregel: Was „in Benutzung“ ist, lässt sich nur mit [`DeleteMode`] löschen (`integrity.rs`).
//! * Alle mehrstufigen Änderungen laufen atomar (SAVEPOINT) und lassen sich in einer äußeren Transaktion verschachteln.
//!
//! Die Tickets stehen in eigenen Untermodulen mit eigener Fehlerart ([`ticket_error::TicketError`], zweisprachig):
//! * `tickets` (Anlegen, Ändern, Löschen, Markierung/Stern, Eltern/Kinder, Fortschritt, Reihenfolge), `values` (Feldwerte),
//!   `transition` (Statuswechsel mit allen Regeln), `clone_move` (Klonen, Typ ändern, verschieben), `ticket_list` (einfache
//!   Abfrage mit Seiten);
//! * `links`, `comments`, `time_entries`, `history` (Quelle und Verlaufstexte), `todo_bridge` (einfache Todo-Liste lesen und in
//!   Tickets umwandeln);
//! * `ops` (Rückgängig/Wiederholen über `undo::History`);
//! * `agent` (die MCP-Werkzeuge für Todo-Liste und Tickets: JSON hinein, JSON heraus, mit denselben Regeln wie die Oberfläche).

// Die Schicht ist vollständig, die Oberfläche und die Tickets (nächste Schritte) nutzen sie erst nach und nach.
#![allow(dead_code)]

pub mod agent;
pub mod clone_move;
pub mod comments;
pub mod defaults;
pub mod fields;
pub mod history;
pub mod icons;
pub mod integrity;
pub mod link_types;
pub mod links;
pub mod names;
pub mod ops;
pub mod projects;
pub mod quick;
pub mod query_help;
pub mod schema;
pub mod sprints;
pub mod statuses;
pub mod summary;
pub mod template;
pub mod ticket_error;
pub mod ticket_list;
pub mod ticket_schema;
pub mod tickets;
pub mod time_entries;
pub mod todo_bridge;
pub mod transition;
pub mod types;
pub mod values;
pub mod views;
pub mod work_kinds;
pub mod workflow;

pub(crate) mod common;

#[cfg(test)]
mod ticket_integrity_tests;
#[cfg(test)]
mod usage_tests;

use crate::state::Lang;

/// Was an einem Feld nicht stimmt (Optionen, Standardwert, Regeln).
#[derive(Debug, PartialEq, Clone)]
pub enum OptionsProblem {
    /// Zwei Dropdown-/Mehrfachauswahl-Werte heißen gleich.
    DuplicateValue,
    /// Ein Wert hat weder Schlüssel noch Namen.
    EmptyValueName,
    /// Farbe eines Werts ist kein „#RRGGBB“.
    ValueColor,
    /// Zahl: Minimum größer als Maximum (oder Wert keine endliche Zahl).
    MinGreaterMax,
    /// Zahl: Nachkommastellen außerhalb von 0 bis 6.
    Decimals,
    /// Einheit zu lang.
    Unit,
    /// Text: Längenbegrenzung unsinnig (0 oder zu groß).
    TextLength,
    /// Rich-Text: Zeilenzahl außerhalb von 1 bis 50.
    Rows,
    /// Ticket-Feld: Ziel-Typ gehört nicht zum Projekt des Typs.
    TargetType,
    /// Zwei Werte haben dieselbe Id.
    DuplicateId,
}

/// Was an einer Workflow-Regel nicht stimmt.
#[derive(Debug, PartialEq, Clone)]
pub enum RuleProblem {
    /// Das Feld gehört nicht zum Typ des Workflows.
    FieldNotInType,
    /// Vergleichsart passt nicht zum Feldtyp (z. B. „größer“ bei Text).
    Operator,
    /// Der Wert passt nicht zum Feldtyp.
    Value,
}

#[derive(Debug, PartialEq, Clone)]
pub enum TemplateProblem {
    /// Kein Paneldeck-Typvorlagen-Dokument.
    Format,
    /// Neuere Format-Version als bekannt.
    Version { found: u32, supported: u32 },
    /// Das Dokument verweist auf etwas, das es nicht enthält (Status, Typ, Feld).
    Reference(String),
    /// Ein Wert im Dokument ist ungültig (z. B. unbekannter Feldtyp, Farbe).
    Invalid(String),
    /// JSON nicht lesbar.
    Json(String),
}

#[derive(Debug, PartialEq, Clone)]
pub enum PlanError {
    /// Projektschlüssel: 2–10 Zeichen, Großbuchstaben/Ziffern, beginnt mit einem Buchstaben.
    KeyInvalid,
    KeyTaken,
    /// Farbe ist kein „#RRGGBB“.
    ColorInvalid,
    NameEmpty,
    NameTooLong { max: usize },
    /// Name (in einer der beiden Sprachen) ist im selben Bereich schon vergeben.
    NameTaken,
    NotFound,
    /// System-Projekt/-Typ/-Verknüpfungstyp: nicht änderbar bzw. nicht löschbar.
    SystemProtected,
    /// In Benutzung (von `count` Tickets/Werten); löschen nur mit [`DeleteMode`].
    InUse { count: i64 },
    /// Ersatz beim Löschen/Migrieren unbrauchbar (gleicher Eintrag, anderes Projekt, falscher Feldtyp …).
    ReplacementInvalid,
    /// Icon nicht im festen Satz.
    IconInvalid,
    /// Die Reihenfolge-Liste enthält nicht genau die vorhandenen Einträge.
    ReorderMismatch,
    /// Hierarchie: Eltern-Typ aus einem anderen Projekt oder System-Typ.
    HierarchyForeign,
    /// Hierarchie: die Regeln wären zyklisch (auch Typ unter sich selbst).
    HierarchyCycle,
    /// Der Typ hat keinen Workflow (Statusfeld).
    WorkflowOff,
    /// Status gehört nicht zum Projekt oder nicht zum Workflow des Typs.
    StatusNotInWorkflow,
    /// Übergang von einem Status in sich selbst.
    TransitionSame,
    Rule(RuleProblem),
    Options(OptionsProblem),
    /// Standardwert (bzw. gesetzter Wert) passt nicht zum Feldtyp und seinen Optionen.
    ValueInvalid,
    /// Verknüpfungstyp: Namen/Richtungen unstimmig (z. B. fehlende Rückrichtung bei gerichtetem Typ).
    LinkTypeInvalid,
    Template(TemplateProblem),
    Db(String),
}

impl From<rusqlite::Error> for PlanError {
    fn from(e: rusqlite::Error) -> Self {
        PlanError::Db(e.to_string())
    }
}

impl PlanError {
    pub fn message(&self, lang: Lang) -> String {
        let s = |de: &'static str, en: &'static str| lang.t(de, en).to_string();
        match self {
            PlanError::KeyInvalid => s(
                "Der Schlüssel braucht 2 bis 10 Zeichen (Großbuchstaben und Ziffern) und muss mit einem Buchstaben beginnen.",
                "The key needs 2 to 10 characters (capital letters and digits) and must start with a letter.",
            ),
            PlanError::KeyTaken => s("Dieser Schlüssel ist schon vergeben.", "This key is already in use."),
            PlanError::ColorInvalid => s("Die Farbe muss die Form #RRGGBB haben.", "The color must look like #RRGGBB."),
            PlanError::NameEmpty => s("Der Name darf nicht leer sein.", "The name must not be empty."),
            PlanError::NameTooLong { max } => match lang {
                Lang::De => format!("Der Name ist zu lang (höchstens {max} Zeichen)."),
                Lang::En => format!("The name is too long (at most {max} characters)."),
            },
            PlanError::NameTaken => s("Dieser Name ist schon vergeben.", "This name is already in use."),
            PlanError::NotFound => s("Der Eintrag existiert nicht mehr.", "The entry no longer exists."),
            PlanError::SystemProtected => s("Dieser Systemeintrag lässt sich nicht ändern oder löschen.", "This system entry cannot be changed or deleted."),
            PlanError::InUse { count } => match lang {
                Lang::De => format!("Der Eintrag wird noch verwendet ({count}x). Zum Löschen ist ein Ersatz oder bewusstes Verwerfen nötig."),
                Lang::En => format!("The entry is still in use ({count}x). Deleting it needs a replacement or an explicit discard."),
            },
            PlanError::ReplacementInvalid => s("Der gewählte Ersatz ist nicht zulässig.", "The chosen replacement is not allowed."),
            PlanError::IconInvalid => s("Dieses Icon gibt es nicht.", "This icon does not exist."),
            PlanError::ReorderMismatch => s("Die Reihenfolge passt nicht zu den vorhandenen Einträgen.", "The order does not match the existing entries."),
            PlanError::HierarchyForeign => s("Der übergeordnete Typ muss zum selben Projekt gehören.", "The parent type must belong to the same project."),
            PlanError::HierarchyCycle => s("Diese Hierarchie wäre ein Kreis.", "This hierarchy would form a cycle."),
            PlanError::WorkflowOff => s("Dieser Typ hat keinen Workflow.", "This type has no workflow."),
            PlanError::StatusNotInWorkflow => s("Der Status gehört nicht zum Workflow dieses Typs.", "The status does not belong to this type's workflow."),
            PlanError::TransitionSame => s("Ein Übergang braucht zwei verschiedene Status.", "A transition needs two different statuses."),
            PlanError::Rule(RuleProblem::FieldNotInType) => s("Das Feld gehört nicht zu diesem Typ.", "The field does not belong to this type."),
            PlanError::Rule(RuleProblem::Operator) => s("Diese Vergleichsart passt nicht zum Feld.", "This comparison does not fit the field."),
            PlanError::Rule(RuleProblem::Value) => s("Der Wert passt nicht zum Feld.", "The value does not fit the field."),
            PlanError::Options(p) => match p {
                OptionsProblem::DuplicateValue => s("Zwei Werte heißen gleich.", "Two values have the same name."),
                OptionsProblem::EmptyValueName => s("Jeder Wert braucht einen Namen.", "Every value needs a name."),
                OptionsProblem::ValueColor => s("Die Farbe eines Werts muss die Form #RRGGBB haben.", "A value's color must look like #RRGGBB."),
                OptionsProblem::MinGreaterMax => s("Das Minimum darf nicht größer als das Maximum sein.", "The minimum must not exceed the maximum."),
                OptionsProblem::Decimals => s("Nachkommastellen: 0 bis 6.", "Decimal places: 0 to 6."),
                OptionsProblem::Unit => s("Die Einheit ist zu lang.", "The unit is too long."),
                OptionsProblem::TextLength => s("Die maximale Textlänge ist ungültig.", "The maximum text length is invalid."),
                OptionsProblem::Rows => s("Die Zeilenzahl muss zwischen 1 und 50 liegen.", "The number of rows must be between 1 and 50."),
                OptionsProblem::TargetType => s("Ein Ziel-Typ gehört nicht zu diesem Projekt.", "A target type does not belong to this project."),
                OptionsProblem::DuplicateId => s("Zwei Werte haben dieselbe Kennung.", "Two values share the same id."),
            },
            PlanError::ValueInvalid => s("Der Wert passt nicht zum Feldtyp.", "The value does not fit the field type."),
            PlanError::LinkTypeInvalid => s("Die Namen des Verknüpfungstyps sind unvollständig.", "The link type's names are incomplete."),
            PlanError::Template(t) => match t {
                TemplateProblem::Format => s("Das ist keine Typvorlage von Paneldeck.", "This is not a Paneldeck type template."),
                TemplateProblem::Version { found, supported } => format!(
                    "{} {found}, {} {supported}.",
                    lang.t("Die Vorlage hat die Format-Version", "The template has format version"),
                    lang.t("bekannt ist nur bis", "but only up to")
                ),
                TemplateProblem::Reference(r) => format!("{}: {r}", lang.t("Die Vorlage verweist auf etwas Fehlendes", "The template refers to something missing")),
                TemplateProblem::Invalid(r) => format!("{}: {r}", lang.t("Ungültiger Wert in der Vorlage", "Invalid value in the template")),
                TemplateProblem::Json(r) => format!("{}: {r}", lang.t("Die Vorlage ist nicht lesbar", "The template cannot be read")),
            },
            PlanError::Db(e) => format!("{}: {e}", lang.t("Datenbankfehler", "Database error")),
        }
    }
}

pub type Res<T> = Result<T, PlanError>;

#[cfg(test)]
pub(crate) mod testutil {
    use rusqlite::Connection;

    /// Frische Datenbank im Speicher mit allen Migrationen.
    pub fn db() -> Connection {
        crate::db::open_memory()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_error_has_both_languages() {
        let all = vec![
            PlanError::KeyInvalid,
            PlanError::KeyTaken,
            PlanError::ColorInvalid,
            PlanError::NameEmpty,
            PlanError::NameTooLong { max: 80 },
            PlanError::NameTaken,
            PlanError::NotFound,
            PlanError::SystemProtected,
            PlanError::InUse { count: 3 },
            PlanError::ReplacementInvalid,
            PlanError::IconInvalid,
            PlanError::ReorderMismatch,
            PlanError::HierarchyForeign,
            PlanError::HierarchyCycle,
            PlanError::WorkflowOff,
            PlanError::StatusNotInWorkflow,
            PlanError::TransitionSame,
            PlanError::Rule(RuleProblem::FieldNotInType),
            PlanError::Rule(RuleProblem::Operator),
            PlanError::Rule(RuleProblem::Value),
            PlanError::Options(OptionsProblem::DuplicateValue),
            PlanError::Options(OptionsProblem::EmptyValueName),
            PlanError::Options(OptionsProblem::ValueColor),
            PlanError::Options(OptionsProblem::MinGreaterMax),
            PlanError::Options(OptionsProblem::Decimals),
            PlanError::Options(OptionsProblem::Unit),
            PlanError::Options(OptionsProblem::TextLength),
            PlanError::Options(OptionsProblem::Rows),
            PlanError::Options(OptionsProblem::TargetType),
            PlanError::Options(OptionsProblem::DuplicateId),
            PlanError::ValueInvalid,
            PlanError::LinkTypeInvalid,
            PlanError::Template(TemplateProblem::Format),
            PlanError::Template(TemplateProblem::Version { found: 2, supported: 1 }),
            PlanError::Template(TemplateProblem::Reference("x".into())),
            PlanError::Template(TemplateProblem::Invalid("x".into())),
            PlanError::Template(TemplateProblem::Json("x".into())),
            PlanError::Db("x".into()),
        ];
        for e in all {
            let (de, en) = (e.message(Lang::De), e.message(Lang::En));
            assert!(!de.is_empty() && !en.is_empty(), "{e:?}");
            assert_ne!(de, en, "{e:?}: deutsch und englisch sollten sich unterscheiden");
        }
        assert!(PlanError::InUse { count: 7 }.message(Lang::En).contains('7'));
        assert!(PlanError::NameTooLong { max: 80 }.message(Lang::De).contains("80"));
        assert!(PlanError::NameTooLong { max: 80 }.message(Lang::En).contains("80"));
    }
}
