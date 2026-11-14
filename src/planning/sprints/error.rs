//! Fehler der Sprint-/Versions-Schicht (zweisprachig, im Stil von `PlanError` und `TicketError`).

use crate::planning::ticket_error::TicketError;
use crate::planning::PlanError;
use crate::state::Lang;

#[derive(Debug, Clone, PartialEq)]
pub enum SprintError {
    /// Fehler aus der Konfigurationsschicht (nicht gefunden, Name, „in Benutzung“, Datenbank …).
    Plan(PlanError),
    /// Das Projekt nutzt keine Sprints (Projekteinstellung „Sprints“ ist aus).
    SprintsDisabled,
    /// Das Projekt nutzt keine Versionen/Meilensteine (Projekteinstellung ist aus).
    VersionsDisabled,
    /// Ein Datum ist kein `JJJJ-MM-TT`.
    DateInvalid,
    /// Das Ende liegt vor dem Start.
    DatesOrder,
    /// Start oder Ende fehlt (nur Ende ohne Start; aktive Sprints brauchen beide).
    DatesIncomplete,
    /// Der Zeitraum ist länger als `max` Tage.
    DatesTooLong { max: i64 },
    /// Dauer außerhalb von 1 bis `max` Arbeitstagen.
    DaysInvalid { max: u32 },
    /// Ein Text (Ziel, Beschreibung) ist zu lang.
    TextTooLong { max: usize },
    /// Der Kalender hat keinen Arbeitstag, das Ende lässt sich daher nicht aus der Dauer berechnen.
    NoWorkDays,
    /// Der Sprint ist abgeschlossen und nicht mehr änderbar.
    SprintCompleted,
    /// Der Vorgang geht nur mit einem geplanten Sprint (starten, löschen).
    NotPlanned,
    /// Der Vorgang geht nur mit dem aktiven Sprint (abschließen).
    NotActive,
    /// Im Projekt läuft schon ein anderer Sprint.
    AnotherActive { sprint_id: i64 },
    /// Das Ticket gehört zu einem anderen Projekt als der Sprint.
    TicketOtherProject { ticket_id: i64 },
    /// Das Ziel für die unerledigten Tickets ist unbrauchbar (derselbe, abgeschlossener oder fremder Sprint).
    LeftoverInvalid,
}

pub type SRes<T> = Result<T, SprintError>;

impl From<PlanError> for SprintError {
    fn from(e: PlanError) -> Self {
        SprintError::Plan(e)
    }
}

impl From<rusqlite::Error> for SprintError {
    fn from(e: rusqlite::Error) -> Self {
        SprintError::Plan(PlanError::Db(e.to_string()))
    }
}

impl From<TicketError> for SprintError {
    fn from(e: TicketError) -> Self {
        match e {
            TicketError::Plan(p) => SprintError::Plan(p),
            other => SprintError::Plan(PlanError::Db(other.message(crate::state::lang()))),
        }
    }
}

impl SprintError {
    pub fn not_found() -> Self {
        SprintError::Plan(PlanError::NotFound)
    }

    pub fn is_not_found(&self) -> bool {
        matches!(self, SprintError::Plan(PlanError::NotFound))
    }

    pub fn message(&self, lang: Lang) -> String {
        let s = |de: &'static str, en: &'static str| lang.t(de, en).to_string();
        match self {
            SprintError::Plan(p) => p.message(lang),
            SprintError::SprintsDisabled => s("Dieses Projekt nutzt keine Sprints. Sie lassen sich in den Projekteinstellungen einschalten.", "This project does not use sprints. You can turn them on in the project settings."),
            SprintError::VersionsDisabled => s(
                "Dieses Projekt nutzt keine Versionen und Meilensteine. Sie lassen sich in den Projekteinstellungen einschalten.",
                "This project does not use versions and milestones. You can turn them on in the project settings.",
            ),
            SprintError::DateInvalid => s("Das Datum muss die Form JJJJ-MM-TT haben.", "The date must look like YYYY-MM-DD."),
            SprintError::DatesOrder => s("Das Ende darf nicht vor dem Start liegen.", "The end must not be before the start."),
            SprintError::DatesIncomplete => s("Start und Ende müssen beide gesetzt sein.", "Both start and end must be set."),
            SprintError::DatesTooLong { max } => match lang {
                Lang::De => format!("Der Zeitraum darf höchstens {max} Tage lang sein."),
                Lang::En => format!("The period must not be longer than {max} days."),
            },
            SprintError::DaysInvalid { max } => match lang {
                Lang::De => format!("Die Dauer muss zwischen 1 und {max} Arbeitstagen liegen."),
                Lang::En => format!("The duration must be between 1 and {max} work days."),
            },
            SprintError::TextTooLong { max } => match lang {
                Lang::De => format!("Der Text ist zu lang (höchstens {max} Zeichen)."),
                Lang::En => format!("The text is too long (at most {max} characters)."),
            },
            SprintError::NoWorkDays => s("Der Kalender hat keinen Arbeitstag, daher lässt sich das Ende nicht berechnen.", "The calendar has no work day, so the end cannot be calculated."),
            SprintError::SprintCompleted => s("Der Sprint ist abgeschlossen und lässt sich nicht mehr ändern.", "The sprint is completed and can no longer be changed."),
            SprintError::NotPlanned => s("Das geht nur mit einem geplanten Sprint.", "This only works with a planned sprint."),
            SprintError::NotActive => s("Das geht nur mit dem aktiven Sprint.", "This only works with the active sprint."),
            SprintError::AnotherActive { .. } => s("In diesem Projekt läuft schon ein Sprint. Schließe ihn zuerst ab.", "A sprint is already running in this project. Complete it first."),
            SprintError::TicketOtherProject { .. } => s("Das Ticket gehört zu einem anderen Projekt als der Sprint.", "The ticket belongs to a different project than the sprint."),
            SprintError::LeftoverInvalid => s(
                "Dorthin lassen sich die unerledigten Tickets nicht verschieben (nötig: ein geplanter Sprint desselben Projekts).",
                "The unfinished tickets cannot be moved there (a planned sprint of the same project is required).",
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_error_has_both_languages() {
        let all = vec![
            SprintError::Plan(PlanError::NotFound),
            SprintError::SprintsDisabled,
            SprintError::VersionsDisabled,
            SprintError::DateInvalid,
            SprintError::DatesOrder,
            SprintError::DatesIncomplete,
            SprintError::DatesTooLong { max: 4000 },
            SprintError::DaysInvalid { max: 366 },
            SprintError::TextTooLong { max: 500 },
            SprintError::NoWorkDays,
            SprintError::SprintCompleted,
            SprintError::NotPlanned,
            SprintError::NotActive,
            SprintError::AnotherActive { sprint_id: 3 },
            SprintError::TicketOtherProject { ticket_id: 4 },
            SprintError::LeftoverInvalid,
        ];
        for e in all {
            let (de, en) = (e.message(Lang::De), e.message(Lang::En));
            assert!(!de.is_empty() && !en.is_empty(), "{e:?}");
            assert_ne!(de, en, "{e:?}: deutsch und englisch sollten sich unterscheiden");
        }
        assert!(SprintError::DaysInvalid { max: 366 }.message(Lang::En).contains("366"));
        assert!(SprintError::TextTooLong { max: 500 }.message(Lang::De).contains("500"));
    }

    #[test]
    fn conversions_keep_the_source() {
        let e: SprintError = PlanError::NameTaken.into();
        assert_eq!(e, SprintError::Plan(PlanError::NameTaken));
        assert!(SprintError::not_found().is_not_found());
        assert!(!SprintError::SprintsDisabled.is_not_found());
        let t: SprintError = TicketError::not_found().into();
        assert!(t.is_not_found());
        let t: SprintError = TicketError::TitleEmpty.into();
        assert!(matches!(t, SprintError::Plan(PlanError::Db(_))));
    }
}
