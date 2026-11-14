//! Fehler der Ticket-Schicht (zweisprachig, im Stil von [`PlanError`]) und die Hilfe für atomare Abläufe.
//!
//! Fehler, die zu einem Feld gehören, tragen dessen Bezeichnung in beiden Sprachen ([`FieldLabel`]), damit die Meldung
//! (auch für Agenten) die Felder beim Namen nennt und die Oberfläche bei [`TicketError::RequiredMissing`] und
//! [`TicketError::TransitionNeedsFields`] gezielt einen Dialog für genau diese Felder öffnen kann.

use super::fields::Field;
use super::PlanError;
use crate::calc::duration::DurationError;
use crate::state::Lang;
use rusqlite::Connection;

/// Ein Feld, wie es in Fehlermeldungen vorkommt: Kennung und Bezeichnung in beiden Sprachen.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldLabel {
    pub id: i64,
    pub en: String,
    pub de: String,
}

impl FieldLabel {
    pub fn of(f: &Field) -> Self {
        FieldLabel { id: f.id, en: f.display(Lang::En), de: f.display(Lang::De) }
    }

    pub fn text(&self, lang: Lang) -> &str {
        match lang {
            Lang::En => &self.en,
            Lang::De => &self.de,
        }
    }
}

fn list(v: &[FieldLabel], lang: Lang) -> String {
    v.iter().map(|f| f.text(lang)).collect::<Vec<_>>().join(", ")
}

#[derive(Debug, Clone, PartialEq)]
pub enum TicketError {
    /// Fehler aus der Konfigurationsschicht (nicht gefunden, System-Eintrag, Datenbank …).
    Plan(PlanError),
    TitleEmpty,
    TitleTooLong { max: usize },
    /// Der Typ gehört nicht zum Projekt des Tickets.
    TypeNotInProject,
    /// Das Feld gehört nicht zum Typ des Tickets.
    FieldNotInType { field_id: i64 },
    /// Der Wert passt nicht zum Feld (Feldtyp, Optionen, Grenzen).
    ValueInvalid(FieldLabel),
    /// Ein Verweis (Ticket-Feld) ist ungültig: Ziel fehlt, anderes Projekt, falscher Typ, das Ticket selbst.
    RefInvalid(FieldLabel),
    /// Pflichtfelder fehlen.
    RequiredMissing(Vec<FieldLabel>),
    ParentSelf,
    ParentCycle,
    ParentOtherProject,
    /// Die Hierarchie-Regeln erlauben diesen Typ nicht unter dem Typ des Eltern-Tickets.
    ParentNotAllowed,
    /// Der Typ des Tickets hat keinen Workflow.
    NoWorkflow,
    /// Der Zielstatus gehört nicht zum Workflow des Tickets.
    StatusNotInWorkflow,
    /// Die Übergangsmatrix erlaubt diesen Wechsel nicht.
    TransitionNotAllowed,
    /// Eine Bedingung des Übergangs sperrt ihn.
    ConditionsNotMet,
    /// Pflichtfelder des Übergangs sind leer; die Werte lassen sich in derselben Operation mitgeben.
    TransitionNeedsFields(Vec<FieldLabel>),
    /// Die Feld-Zuordnung beim Typwechsel ist unzulässig.
    MappingInvalid,
    /// Der Verknüpfungstyp erlaubt dieses Ziel nicht.
    LinkTargetMismatch,
    LinkSelf,
    LinkDuplicate,
    /// Das Ziel-Ticket existiert nicht.
    LinkTargetMissing,
    /// Das Ziel ist leer oder unzulässig (URL ohne Adresse, Branch ohne Namen, leere Terminal-Kennung).
    LinkTargetInvalid,
    CommentEmpty,
    CommentTooLong { max: usize },
    DurationInvalid(DurationError),
    DurationZero,
    /// Der Startzeitpunkt ist nicht darstellbar (z. B. Dauer reicht vor das Jahr 1).
    StartInvalid,
    /// Die Art der Arbeit gehört nicht zum Projekt des Tickets.
    WorkKindInvalid,
    /// Das Anlegen lässt sich nicht zurücknehmen, weil das Ticket inzwischen weitere Daten hat.
    CannotUndoCreate,
    /// Die Zuordnung zu Sprints lässt sich nicht zurücknehmen oder wiederholen, weil sich Sprints oder Zeilen inzwischen geändert haben.
    SprintChanged,
    /// Die Todo-Karte wurde schon in ein Ticket umgewandelt.
    TodoAlreadyConverted { ticket_id: i64 },
    Db(String),
}

pub type TRes<T> = Result<T, TicketError>;

impl From<PlanError> for TicketError {
    fn from(e: PlanError) -> Self {
        TicketError::Plan(e)
    }
}

impl From<rusqlite::Error> for TicketError {
    fn from(e: rusqlite::Error) -> Self {
        TicketError::Db(e.to_string())
    }
}

impl TicketError {
    /// `NotFound` der Konfigurationsschicht (Ticket, Feld, Typ … existiert nicht).
    pub fn not_found() -> Self {
        TicketError::Plan(PlanError::NotFound)
    }

    pub fn is_not_found(&self) -> bool {
        matches!(self, TicketError::Plan(PlanError::NotFound))
    }

    pub fn message(&self, lang: Lang) -> String {
        let s = |de: &'static str, en: &'static str| lang.t(de, en).to_string();
        match self {
            TicketError::Plan(p) => p.message(lang),
            TicketError::TitleEmpty => s("Der Titel darf nicht leer sein.", "The title must not be empty."),
            TicketError::TitleTooLong { max } => match lang {
                Lang::De => format!("Der Titel ist zu lang (höchstens {max} Zeichen)."),
                Lang::En => format!("The title is too long (at most {max} characters)."),
            },
            TicketError::TypeNotInProject => s("Der Typ gehört nicht zu diesem Projekt.", "The type does not belong to this project."),
            TicketError::FieldNotInType { .. } => s("Das Feld gehört nicht zum Typ dieses Tickets.", "The field does not belong to this ticket's type."),
            TicketError::ValueInvalid(f) => match lang {
                Lang::De => format!("Der Wert passt nicht zum Feld {}.", lang.q(&f.de)),
                Lang::En => format!("The value does not fit the field {}.", lang.q(&f.en)),
            },
            TicketError::RefInvalid(f) => match lang {
                Lang::De => format!("Der Verweis im Feld {} ist ungültig (Ziel fehlt, anderes Projekt, nicht erlaubter Typ oder das Ticket selbst).", lang.q(&f.de)),
                Lang::En => format!("The reference in the field {} is invalid (missing target, other project, type not allowed, or the ticket itself).", lang.q(&f.en)),
            },
            TicketError::RequiredMissing(v) => match lang {
                Lang::De => format!("Pflichtfelder fehlen: {}.", list(v, lang)),
                Lang::En => format!("Required fields are missing: {}.", list(v, lang)),
            },
            TicketError::ParentSelf => s("Ein Ticket kann nicht sein eigenes Eltern-Ticket sein.", "A ticket cannot be its own parent."),
            TicketError::ParentCycle => s("Das Eltern-Ticket würde einen Kreis in der Hierarchie bilden.", "The parent would form a cycle in the hierarchy."),
            TicketError::ParentOtherProject => s("Das Eltern-Ticket muss im selben Projekt liegen.", "The parent must be in the same project."),
            TicketError::ParentNotAllowed => s("Die Hierarchie-Regeln erlauben diesen Typ nicht unter dem Typ des Eltern-Tickets.", "The hierarchy rules do not allow this type under the parent's type."),
            TicketError::NoWorkflow => s("Der Typ dieses Tickets hat keinen Workflow und damit keinen Status.", "This ticket's type has no workflow and therefore no status."),
            TicketError::StatusNotInWorkflow => s("Der Status gehört nicht zum Workflow dieses Tickets.", "The status does not belong to this ticket's workflow."),
            TicketError::TransitionNotAllowed => s("Dieser Statuswechsel ist im Workflow nicht erlaubt.", "This status change is not allowed by the workflow."),
            TicketError::ConditionsNotMet => s("Eine Bedingung des Übergangs sperrt diesen Statuswechsel.", "A condition of the transition blocks this status change."),
            TicketError::TransitionNeedsFields(v) => match lang {
                Lang::De => format!("Für diesen Statuswechsel müssen Felder ausgefüllt sein: {}.", list(v, lang)),
                Lang::En => format!("This status change requires these fields to be filled in: {}.", list(v, lang)),
            },
            TicketError::MappingInvalid => s("Die Zuordnung der Felder beim Typwechsel ist unzulässig.", "The field mapping for the type change is not allowed."),
            TicketError::LinkTargetMismatch => s("Dieser Verknüpfungstyp erlaubt kein Ziel dieser Art.", "This link type does not allow a target of this kind."),
            TicketError::LinkSelf => s("Ein Ticket lässt sich nicht mit sich selbst verknüpfen.", "A ticket cannot be linked to itself."),
            TicketError::LinkDuplicate => s("Diese Verknüpfung gibt es schon.", "This link already exists."),
            TicketError::LinkTargetMissing => s("Das Ziel-Ticket existiert nicht.", "The target ticket does not exist."),
            TicketError::LinkTargetInvalid => s("Das Ziel der Verknüpfung ist leer oder ungültig.", "The link target is empty or invalid."),
            TicketError::CommentEmpty => s("Ein Kommentar darf nicht leer sein.", "A comment must not be empty."),
            TicketError::CommentTooLong { max } => match lang {
                Lang::De => format!("Der Kommentar ist zu lang (höchstens {max} Zeichen)."),
                Lang::En => format!("The comment is too long (at most {max} characters)."),
            },
            TicketError::DurationInvalid(e) => e.message(lang),
            TicketError::DurationZero => s("Die Dauer muss größer als null sein.", "The duration must be greater than zero."),
            TicketError::StartInvalid => s("Der Startzeitpunkt ist ungültig.", "The start time is invalid."),
            TicketError::WorkKindInvalid => s("Die Art der Arbeit gehört nicht zu diesem Projekt.", "The kind of work does not belong to this project."),
            TicketError::CannotUndoCreate => s(
                "Das Anlegen lässt sich nicht zurücknehmen: Das Ticket hat inzwischen Kommentare, Zeiten, Verknüpfungen oder Kinder.",
                "Creating cannot be undone: the ticket now has comments, time entries, links or children.",
            ),
            TicketError::SprintChanged => s(
                "Die Sprint-Zuordnung lässt sich nicht mehr zurücknehmen oder wiederholen: Der Sprint wurde inzwischen gestartet, abgeschlossen, gelöscht oder anders geändert.",
                "The sprint assignment can no longer be undone or redone: the sprint has been started, completed, deleted or changed in the meantime.",
            ),
            TicketError::TodoAlreadyConverted { .. } => s("Diese Todo-Karte wurde schon in ein Ticket umgewandelt.", "This todo card has already been converted to a ticket."),
            TicketError::Db(e) => format!("{}: {e}", lang.t("Datenbankfehler", "Database error")),
        }
    }
}

/// Führt `f` atomar aus (SAVEPOINT): bei einem Fehler wird alles zurückgenommen. Lässt sich verschachteln und in einer
/// äußeren Transaktion benutzen.
pub(crate) fn atomic<T>(conn: &Connection, f: impl FnOnce() -> TRes<T>) -> TRes<T> {
    conn.execute_batch("SAVEPOINT ticket_op")?;
    match f() {
        Ok(v) => {
            conn.execute_batch("RELEASE ticket_op")?;
            Ok(v)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK TO ticket_op; RELEASE ticket_op");
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calc::duration::{DurationErrorKind, DurationError};

    fn label() -> FieldLabel {
        FieldLabel { id: 1, en: "Description".into(), de: "Beschreibung".into() }
    }

    #[test]
    fn every_error_has_both_languages() {
        let all = vec![
            TicketError::Plan(PlanError::NotFound),
            TicketError::TitleEmpty,
            TicketError::TitleTooLong { max: 255 },
            TicketError::TypeNotInProject,
            TicketError::FieldNotInType { field_id: 1 },
            TicketError::ValueInvalid(label()),
            TicketError::RefInvalid(label()),
            TicketError::RequiredMissing(vec![label()]),
            TicketError::ParentSelf,
            TicketError::ParentCycle,
            TicketError::ParentOtherProject,
            TicketError::ParentNotAllowed,
            TicketError::NoWorkflow,
            TicketError::StatusNotInWorkflow,
            TicketError::TransitionNotAllowed,
            TicketError::ConditionsNotMet,
            TicketError::TransitionNeedsFields(vec![label()]),
            TicketError::MappingInvalid,
            TicketError::LinkTargetMismatch,
            TicketError::LinkSelf,
            TicketError::LinkDuplicate,
            TicketError::LinkTargetMissing,
            TicketError::LinkTargetInvalid,
            TicketError::CommentEmpty,
            TicketError::CommentTooLong { max: 10 },
            TicketError::DurationInvalid(DurationError { kind: DurationErrorKind::Empty, pos: 0 }),
            TicketError::DurationZero,
            TicketError::StartInvalid,
            TicketError::WorkKindInvalid,
            TicketError::CannotUndoCreate,
            TicketError::SprintChanged,
            TicketError::TodoAlreadyConverted { ticket_id: 4 },
            TicketError::Db("x".into()),
        ];
        for e in all {
            let (de, en) = (e.message(Lang::De), e.message(Lang::En));
            assert!(!de.is_empty() && !en.is_empty(), "{e:?}");
            assert_ne!(de, en, "{e:?}: deutsch und englisch sollten sich unterscheiden");
        }
    }

    #[test]
    fn field_names_appear_in_the_matching_language() {
        let e = TicketError::RequiredMissing(vec![label(), FieldLabel { id: 2, en: "Priority".into(), de: "Priorität".into() }]);
        assert_eq!(e.message(Lang::En), "Required fields are missing: Description, Priority.");
        assert_eq!(e.message(Lang::De), "Pflichtfelder fehlen: Beschreibung, Priorität.");
        assert!(TicketError::ValueInvalid(label()).message(Lang::De).contains("Beschreibung"));
        assert!(TicketError::ValueInvalid(label()).message(Lang::En).contains("Description"));
        assert!(TicketError::TitleTooLong { max: 255 }.message(Lang::En).contains("255"));
    }

    #[test]
    fn conversions_keep_the_source() {
        let e: TicketError = PlanError::SystemProtected.into();
        assert_eq!(e, TicketError::Plan(PlanError::SystemProtected));
        assert!(TicketError::not_found().is_not_found());
        assert!(!TicketError::TitleEmpty.is_not_found());
    }

    #[test]
    fn atomic_rolls_back_on_error_and_nests() {
        let c = crate::db::open_memory();
        let n = |c: &Connection| -> i64 { c.query_row("SELECT COUNT(*) FROM meta WHERE key LIKE 'x%'", [], |r| r.get(0)).unwrap() };
        let r: TRes<()> = atomic(&c, || {
            c.execute("INSERT INTO meta (key, value) VALUES ('x1', '1')", [])?;
            let inner: TRes<()> = atomic(&c, || {
                c.execute("INSERT INTO meta (key, value) VALUES ('x2', '1')", [])?;
                Err(TicketError::TitleEmpty)
            });
            assert!(inner.is_err());
            Ok(())
        });
        assert!(r.is_ok());
        assert_eq!(n(&c), 1, "nur der äußere Teil bleibt");
        let r: TRes<()> = atomic(&c, || {
            c.execute("INSERT INTO meta (key, value) VALUES ('x3', '1')", [])?;
            Err(TicketError::TitleEmpty)
        });
        assert!(r.is_err());
        assert_eq!(n(&c), 1);
    }
}
