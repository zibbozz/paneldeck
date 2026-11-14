//! Fehler und Hinweise von Projektsicherung und Ticket-CSV (alle Meldungen zweisprachig).
//!
//! * [`BackupError`] bricht einen Import ab (er ist atomar: nichts bleibt zurück). Fehler im Inhalt tragen ihre
//!   Fundstelle ([`Where`]): „Ticket 12, Kommentar 3: …“.
//! * [`Warning`] meldet, was beim Import bewusst nicht übernommen wurde (der Import läuft dann trotzdem durch).

use crate::calc::csv::CsvError;
use crate::planning::sprints::SprintError;
use crate::planning::ticket_error::TicketError;
use crate::planning::views::transfer::Unresolved;
use crate::planning::views::ViewError;
use crate::planning::PlanError;
use crate::state::Lang;

/// Worauf sich eine Kennung im Dokument bezieht.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    Status,
    Type,
    Field,
    WorkKind,
    LinkType,
    Ticket,
    Version,
    Milestone,
    Sprint,
}

impl RefKind {
    fn name(self, lang: Lang) -> &'static str {
        match self {
            RefKind::Status => lang.t("Status", "status"),
            RefKind::Type => lang.t("Typ", "type"),
            RefKind::Field => lang.t("Feld", "field"),
            RefKind::WorkKind => lang.t("Art der Arbeit", "kind of work"),
            RefKind::LinkType => lang.t("Verknüpfungstyp", "link type"),
            RefKind::Ticket => lang.t("Ticket", "ticket"),
            RefKind::Version => lang.t("Version", "version"),
            RefKind::Milestone => lang.t("Meilenstein", "milestone"),
            RefKind::Sprint => lang.t("Sprint", "sprint"),
        }
    }
}

/// Was an einem Element des Dokuments nicht stimmt (ohne Fundstelle).
#[derive(Debug, Clone, PartialEq)]
pub enum Problem {
    /// Zwei Tickets haben dieselbe Nummer, oder eine Nummer ist nicht positiv oder zu groß.
    TicketNumber(i64),
    /// Die Reihenfolge-Zahl (`rank`) eines Tickets liegt außerhalb des erlaubten Bereichs.
    Rank(i64),
    /// Status, Typen oder Arten der Arbeit der Struktur heißen doppelt.
    StructureNames,
    /// Interner Abgleich zwischen Struktur und angelegtem Projekt stimmt nicht.
    StructureMismatch,
    /// Das Dokument verweist auf eine Kennung, die es nicht enthält.
    UnknownRef(RefKind, i64),
    /// Das Feld gehört nicht zum Typ des Tickets.
    FieldNotInType,
    /// Der Status gehört nicht zum Workflow des Typs (oder der Typ hat keinen).
    StatusNotAllowed,
    /// Zeitangabe nicht im Format `JJJJ-MM-TTTHH:MM:SS`.
    Timestamp(String),
    /// Unbekannte Quelle im Verlauf.
    Source(String),
    /// Unbekannte Art des Verknüpfungsziels.
    TargetKind(String),
    /// Ein vorhandener Verknüpfungstyp mit demselben Namen hat andere Eigenschaften (Richtung/Ziel).
    LinkTypeDiffers,
    /// Verknüpfungstyp ohne Namen.
    LinkTypeUnnamed,
    /// Dieselbe Verknüpfung steht zweimal im Dokument.
    LinkDuplicate,
    /// Eine Dauer von 0 Minuten.
    Minutes,
    /// Unbekannter Zustand eines Sprints.
    SprintState(String),
    /// Zwei aktive Sprints im Projekt.
    SecondActiveSprint,
    /// Ein Ticket steckt in mehr als einem offenen (geplanten oder aktiven) Sprint (Ticketnummer).
    TicketInTwoSprints(i64),
    /// Dasselbe Ticket ist zweimal gleichzeitig im selben Sprint.
    SprintTicketTwice(i64),
    /// Entfernt, bevor es hinzugefügt wurde.
    RemovedBeforeAdded,
    /// Punkte beim Hinzufügen sind keine Zahl ab 0.
    Points,
}

/// Fundstelle im Dokument.
#[derive(Debug, Clone, PartialEq)]
pub enum Where {
    Project,
    Structure,
    Views,
    LinkType(i64),
    Ticket(i64),
    /// Wert eines Felds an einem Ticket (Ticketnummer, Feld-Nummer).
    Value(i64, i64),
    Comment(i64, usize),
    History(i64, usize),
    Link(i64, usize),
    Time(i64, usize),
    Version(i64),
    Milestone(i64),
    Sprint(i64),
    /// Eintrag der Sprint-Zugehörigkeit (Sprint-Nummer, Position).
    SprintEntry(i64, usize),
}

impl Where {
    pub fn text(&self, lang: Lang) -> String {
        match (self, lang) {
            (Where::Project, l) => l.t("Projekt", "Project").to_string(),
            (Where::Structure, l) => l.t("Typen, Felder und Workflows", "Types, fields and workflows").to_string(),
            (Where::Views, l) => l.t("Boards und Filter", "Boards and filters").to_string(),
            (Where::LinkType(n), Lang::De) => format!("Verknüpfungstyp {n}"),
            (Where::LinkType(n), Lang::En) => format!("Link type {n}"),
            (Where::Ticket(n), Lang::De) => format!("Ticket {n}"),
            (Where::Ticket(n), Lang::En) => format!("Ticket {n}"),
            (Where::Value(t, f), Lang::De) => format!("Ticket {t}, Feld {f}"),
            (Where::Value(t, f), Lang::En) => format!("Ticket {t}, field {f}"),
            (Where::Comment(t, i), Lang::De) => format!("Ticket {t}, Kommentar {}", i + 1),
            (Where::Comment(t, i), Lang::En) => format!("Ticket {t}, comment {}", i + 1),
            (Where::History(t, i), Lang::De) => format!("Ticket {t}, Verlaufseintrag {}", i + 1),
            (Where::History(t, i), Lang::En) => format!("Ticket {t}, history entry {}", i + 1),
            (Where::Link(t, i), Lang::De) => format!("Ticket {t}, Verknüpfung {}", i + 1),
            (Where::Link(t, i), Lang::En) => format!("Ticket {t}, link {}", i + 1),
            (Where::Time(t, i), Lang::De) => format!("Ticket {t}, Zeiteintrag {}", i + 1),
            (Where::Time(t, i), Lang::En) => format!("Ticket {t}, time entry {}", i + 1),
            (Where::Version(n), Lang::De) => format!("Version {n}"),
            (Where::Version(n), Lang::En) => format!("Version {n}"),
            (Where::Milestone(n), Lang::De) => format!("Meilenstein {n}"),
            (Where::Milestone(n), Lang::En) => format!("Milestone {n}"),
            (Where::Sprint(n), Lang::De) => format!("Sprint {n}"),
            (Where::Sprint(n), Lang::En) => format!("Sprint {n}"),
            (Where::SprintEntry(s, i), Lang::De) => format!("Sprint {s}, Ticket-Zeile {}", i + 1),
            (Where::SprintEntry(s, i), Lang::En) => format!("Sprint {s}, ticket entry {}", i + 1),
        }
    }
}

/// Was an der Kopfzeile einer Ticket-CSV nicht stimmt.
#[derive(Debug, Clone, PartialEq)]
pub enum HeaderProblem {
    /// Eine der festen Spalten fehlt oder steht an anderer Stelle (Spalte 1-basiert).
    Fixed { column: usize, expected: String, found: String },
    /// Eine Spalte, die das Format nicht kennt (kein Feld des Projekts).
    Unknown { column: usize, name: String },
    /// Dieselbe Spalte zweimal.
    Duplicate { column: usize, name: String },
}

/// Was an einer Zeile der Ticket-CSV nicht stimmt.
#[derive(Debug, Clone, PartialEq)]
pub enum RowIssue {
    KeyInvalid(String),
    KeyDuplicate(String),
    TypeMissing,
    TypeUnknown(String),
    StatusUnknown(String),
    /// Status gehört nicht zum Workflow des Typs (oder der Typ hat keinen Workflow).
    StatusNotAllowed(String),
    BoolInvalid(String),
    TimestampInvalid(String),
    ParentUnknown(String),
    ParentCycle,
    /// Zelle passt nicht zum Feldtyp (Text der Zelle).
    CellInvalid(String),
    /// Wert nicht in der Werteliste des Felds.
    OptionUnknown(String),
    /// Das Feld gibt es beim Typ des Tickets nicht (Typname).
    FieldNotInType(String),
    /// Verweis auf ein Ticket, das nicht in der Datei steht.
    TicketRefUnknown(String),
    /// Regel der Datenschicht verletzt.
    Ticket(TicketError),
    /// Version oder Meilenstein gibt es im Projekt nicht (Name).
    ReleaseUnknown(String),
    /// Sprint gibt es im Projekt nicht (Name).
    SprintUnknown(String),
    /// Regel der Sprint-Schicht verletzt (z. B. abgeschlossener Sprint).
    Sprint(SprintError),
}

/// Fundstelle und Art eines Problems in der Ticket-CSV. `row` zählt die Zeilen der Tabelle (Kopfzeile = 1).
#[derive(Debug, Clone, PartialEq)]
pub struct RowProblem {
    pub row: usize,
    /// Spaltenüberschrift (leer, wenn die Zeile als Ganzes gemeint ist).
    pub column: String,
    pub issue: RowIssue,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BackupError {
    /// Kein Paneldeck-Dokument des erwarteten Formats.
    Format,
    /// Neuere Format-Version als bekannt.
    Version { found: u32, supported: u32 },
    Json(String),
    /// Das gewünschte Projektkürzel ist schon vergeben – der Aufrufer muss ein neues übergeben.
    KeyTaken(String),
    Plan(PlanError),
    Ticket(TicketError),
    View(ViewError),
    /// Etwas Ungültiges im Dokument, mit Fundstelle.
    At(Where, Problem),
    /// Ein Fehler der Datenschicht an einer Fundstelle.
    AtPlan(Where, PlanError),
    AtTicket(Where, TicketError),
    AtView(Where, ViewError),
    AtSprint(Where, SprintError),
    Csv(CsvError),
    CsvHeader(HeaderProblem),
    /// Probleme in den Zeilen (höchstens [`MAX_ROW_PROBLEMS`]); die Datei wird dann gar nicht importiert.
    CsvRows(Vec<RowProblem>),
    Db(String),
    /// Die Datei ist größer als [`super::MAX_FILE_BYTES`] und wird gar nicht erst gelesen.
    TooLarge { max_mb: u64 },
    /// Der Import wurde abgebrochen (siehe [`super::Progress`]); es wurde nichts angelegt.
    Cancelled,
}

/// So viele Zeilenprobleme werden höchstens gesammelt.
pub const MAX_ROW_PROBLEMS: usize = 50;

impl From<rusqlite::Error> for BackupError {
    fn from(e: rusqlite::Error) -> Self {
        BackupError::Db(e.to_string())
    }
}

impl From<PlanError> for BackupError {
    fn from(e: PlanError) -> Self {
        BackupError::Plan(e)
    }
}

impl From<TicketError> for BackupError {
    fn from(e: TicketError) -> Self {
        BackupError::Ticket(e)
    }
}

impl From<ViewError> for BackupError {
    fn from(e: ViewError) -> Self {
        BackupError::View(e)
    }
}

impl From<CsvError> for BackupError {
    fn from(e: CsvError) -> Self {
        BackupError::Csv(e)
    }
}

impl Problem {
    pub fn message(&self, lang: Lang) -> String {
        let s = |de: &'static str, en: &'static str| lang.t(de, en).to_string();
        match (self, lang) {
            (Problem::TicketNumber(n), Lang::De) => format!("Die Ticketnummer {n} ist ungültig, zu groß oder kommt doppelt vor."),
            (Problem::TicketNumber(n), Lang::En) => format!("The ticket number {n} is invalid, too large or appears twice."),
            (Problem::Rank(n), Lang::De) => format!("Die Reihenfolge-Zahl {n} liegt außerhalb des erlaubten Bereichs."),
            (Problem::Rank(n), Lang::En) => format!("The order value {n} is outside the allowed range."),
            (Problem::StructureNames, _) => s("Status, Typen oder Arten der Arbeit heißen doppelt.", "Statuses, types or kinds of work have duplicate names."),
            (Problem::StructureMismatch, _) => s("Die Struktur lässt sich nicht eindeutig zuordnen.", "The structure cannot be matched unambiguously."),
            (Problem::UnknownRef(k, n), Lang::De) => format!("Verweist auf {} {n}, das im Dokument fehlt.", k.name(lang)),
            (Problem::UnknownRef(k, n), Lang::En) => format!("Refers to {} {n}, which the document does not contain.", k.name(lang)),
            (Problem::FieldNotInType, _) => s("Das Feld gehört nicht zum Typ des Tickets.", "The field does not belong to the ticket's type."),
            (Problem::StatusNotAllowed, _) => s("Der Status gehört nicht zum Workflow des Typs.", "The status does not belong to the type's workflow."),
            (Problem::Timestamp(t), Lang::De) => format!("Ungültige Zeitangabe {} (erwartet JJJJ-MM-TTTHH:MM:SS).", lang.q(t)),
            (Problem::Timestamp(t), Lang::En) => format!("Invalid time {} (expected YYYY-MM-DDTHH:MM:SS).", lang.q(t)),
            (Problem::Source(t), Lang::De) => format!("Unbekannte Quelle {}.", lang.q(t)),
            (Problem::Source(t), Lang::En) => format!("Unknown source {}.", lang.q(t)),
            (Problem::TargetKind(t), Lang::De) => format!("Unbekannte Art des Ziels {}.", lang.q(t)),
            (Problem::TargetKind(t), Lang::En) => format!("Unknown kind of target {}.", lang.q(t)),
            (Problem::LinkTypeDiffers, _) => s(
                "Es gibt schon einen Verknüpfungstyp mit diesem Namen, aber mit anderer Richtung oder anderem Ziel.",
                "A link type with this name already exists but with a different direction or target.",
            ),
            (Problem::LinkTypeUnnamed, _) => s("Der Verknüpfungstyp hat keinen Namen.", "The link type has no name."),
            (Problem::LinkDuplicate, _) => s("Diese Verknüpfung steht doppelt im Dokument.", "This link appears twice in the document."),
            (Problem::Minutes, _) => s("Die Dauer muss größer als null sein.", "The duration must be greater than zero."),
            (Problem::SprintState(t), Lang::De) => format!("Unbekannter Zustand {} des Sprints.", lang.q(t)),
            (Problem::SprintState(t), Lang::En) => format!("Unknown sprint state {}.", lang.q(t)),
            (Problem::SecondActiveSprint, _) => s("Im Projekt gibt es mehr als einen aktiven Sprint.", "The project has more than one active sprint."),
            (Problem::TicketInTwoSprints(n), Lang::De) => format!("Das Ticket {n} steckt in mehr als einem offenen Sprint."),
            (Problem::TicketInTwoSprints(n), Lang::En) => format!("Ticket {n} is in more than one open sprint."),
            (Problem::SprintTicketTwice(n), Lang::De) => format!("Das Ticket {n} steht doppelt im selben Sprint."),
            (Problem::SprintTicketTwice(n), Lang::En) => format!("Ticket {n} appears twice in the same sprint."),
            (Problem::RemovedBeforeAdded, _) => s("Das Ticket wurde vor dem Hinzufügen entfernt.", "The ticket was removed before it was added."),
            (Problem::Points, _) => s("Die Punkte beim Hinzufügen müssen eine Zahl ab 0 sein.", "The points at the time of adding must be a number from 0."),
        }
    }
}

impl HeaderProblem {
    pub fn message(&self, lang: Lang) -> String {
        match (self, lang) {
            (HeaderProblem::Fixed { column, expected, found }, Lang::De) => format!("Kopfzeile, Spalte {column}: erwartet {}, gefunden {}.", lang.q(expected), lang.q(found)),
            (HeaderProblem::Fixed { column, expected, found }, Lang::En) => format!("Header, column {column}: expected {}, found {}.", lang.q(expected), lang.q(found)),
            (HeaderProblem::Unknown { column, name }, Lang::De) => format!("Kopfzeile, Spalte {column}: {} ist keine Spalte dieses Formats (und kein Feld dieses Projekts).", lang.q(name)),
            (HeaderProblem::Unknown { column, name }, Lang::En) => format!("Header, column {column}: {} is not a column of this format (and not a field of this project).", lang.q(name)),
            (HeaderProblem::Duplicate { column, name }, Lang::De) => format!("Kopfzeile, Spalte {column}: {} steht doppelt.", lang.q(name)),
            (HeaderProblem::Duplicate { column, name }, Lang::En) => format!("Header, column {column}: {} appears twice.", lang.q(name)),
        }
    }
}

impl RowIssue {
    pub fn message(&self, lang: Lang) -> String {
        let q = |s: &str| lang.q(s);
        match (self, lang) {
            (RowIssue::KeyInvalid(k), Lang::De) => format!("{} ist kein Ticketschlüssel wie PAN-12.", q(k)),
            (RowIssue::KeyInvalid(k), Lang::En) => format!("{} is not a ticket key like PAN-12.", q(k)),
            (RowIssue::KeyDuplicate(k), Lang::De) => format!("Der Schlüssel {} steht in der Datei mehrfach.", q(k)),
            (RowIssue::KeyDuplicate(k), Lang::En) => format!("The key {} appears several times in the file.", q(k)),
            (RowIssue::TypeMissing, _) => lang.t("Der Typ fehlt.", "The type is missing.").to_string(),
            (RowIssue::TypeUnknown(t), Lang::De) => format!("Den Typ {} gibt es in diesem Projekt nicht.", q(t)),
            (RowIssue::TypeUnknown(t), Lang::En) => format!("This project has no type {}.", q(t)),
            (RowIssue::StatusUnknown(t), Lang::De) => format!("Den Status {} gibt es in diesem Projekt nicht.", q(t)),
            (RowIssue::StatusUnknown(t), Lang::En) => format!("This project has no status {}.", q(t)),
            (RowIssue::StatusNotAllowed(t), Lang::De) => format!("Der Status {} gehört nicht zum Workflow dieses Typs.", q(t)),
            (RowIssue::StatusNotAllowed(t), Lang::En) => format!("The status {} does not belong to this type's workflow.", q(t)),
            (RowIssue::BoolInvalid(t), Lang::De) => format!("{} ist weder true noch false.", q(t)),
            (RowIssue::BoolInvalid(t), Lang::En) => format!("{} is neither true nor false.", q(t)),
            (RowIssue::TimestampInvalid(t), Lang::De) => format!("{} ist keine Zeitangabe JJJJ-MM-TTTHH:MM:SS.", q(t)),
            (RowIssue::TimestampInvalid(t), Lang::En) => format!("{} is not a time like YYYY-MM-DDTHH:MM:SS.", q(t)),
            (RowIssue::ParentUnknown(t), Lang::De) => format!("Das übergeordnete Ticket {} steht nicht in der Datei.", q(t)),
            (RowIssue::ParentUnknown(t), Lang::En) => format!("The parent ticket {} is not in the file.", q(t)),
            (RowIssue::ParentCycle, _) => lang.t("Die Eltern-Verweise bilden einen Kreis.", "The parent references form a cycle.").to_string(),
            (RowIssue::CellInvalid(t), Lang::De) => format!("Der Wert {} passt nicht zum Feldtyp.", q(t)),
            (RowIssue::CellInvalid(t), Lang::En) => format!("The value {} does not fit the field type.", q(t)),
            (RowIssue::OptionUnknown(t), Lang::De) => format!("{} steht nicht in der Werteliste des Felds.", q(t)),
            (RowIssue::OptionUnknown(t), Lang::En) => format!("{} is not in the field's list of values.", q(t)),
            (RowIssue::FieldNotInType(t), Lang::De) => format!("Das Feld gibt es beim Typ {} nicht.", q(t)),
            (RowIssue::FieldNotInType(t), Lang::En) => format!("The type {} has no such field.", q(t)),
            (RowIssue::TicketRefUnknown(t), Lang::De) => format!("Das Ticket {} steht nicht in der Datei.", q(t)),
            (RowIssue::TicketRefUnknown(t), Lang::En) => format!("The ticket {} is not in the file.", q(t)),
            (RowIssue::ReleaseUnknown(t), Lang::De) => format!("Eine Version/einen Meilenstein {} gibt es in diesem Projekt nicht.", q(t)),
            (RowIssue::ReleaseUnknown(t), Lang::En) => format!("This project has no version/milestone {}.", q(t)),
            (RowIssue::SprintUnknown(t), Lang::De) => format!("Den Sprint {} gibt es in diesem Projekt nicht.", q(t)),
            (RowIssue::SprintUnknown(t), Lang::En) => format!("This project has no sprint {}.", q(t)),
            (RowIssue::Sprint(e), l) => e.message(l),
            (RowIssue::Ticket(e), l) => e.message(l),
        }
    }
}

impl RowProblem {
    pub fn message(&self, lang: Lang) -> String {
        let at = match (self.column.is_empty(), lang) {
            (true, Lang::De) => format!("Zeile {}", self.row),
            (true, Lang::En) => format!("Row {}", self.row),
            (false, Lang::De) => format!("Zeile {}, Spalte {}", self.row, lang.q(&self.column)),
            (false, Lang::En) => format!("Row {}, column {}", self.row, lang.q(&self.column)),
        };
        format!("{at}: {}", self.issue.message(lang))
    }
}

impl BackupError {
    pub fn message(&self, lang: Lang) -> String {
        let s = |de: &'static str, en: &'static str| lang.t(de, en).to_string();
        match self {
            BackupError::Format => s("Das ist keine Projektsicherung von Paneldeck.", "This is not a Paneldeck project backup."),
            BackupError::Version { found, supported } => format!(
                "{} {found}, {} {supported}.",
                lang.t("Die Sicherung hat die Format-Version", "The backup has format version"),
                lang.t("bekannt ist nur bis", "but only up to")
            ),
            BackupError::Json(e) => format!("{}: {e}", lang.t("Die Datei ist nicht lesbar", "The file cannot be read")),
            BackupError::KeyTaken(k) => match lang {
                Lang::De => format!("Das Projektkürzel {} ist schon vergeben – bitte ein neues Kürzel wählen.", lang.q(k)),
                Lang::En => format!("The project key {} is already in use – please choose a new key.", lang.q(k)),
            },
            BackupError::Plan(e) => e.message(lang),
            BackupError::Ticket(e) => e.message(lang),
            BackupError::View(e) => e.message(lang),
            BackupError::At(w, p) => format!("{}: {}", w.text(lang), p.message(lang)),
            BackupError::AtPlan(w, e) => format!("{}: {}", w.text(lang), e.message(lang)),
            BackupError::AtTicket(w, e) => format!("{}: {}", w.text(lang), e.message(lang)),
            BackupError::AtView(w, e) => format!("{}: {}", w.text(lang), e.message(lang)),
            BackupError::AtSprint(w, e) => format!("{}: {}", w.text(lang), e.message(lang)),
            BackupError::Csv(e) => e.message(lang),
            BackupError::CsvHeader(h) => h.message(lang),
            BackupError::CsvRows(rows) => {
                let mut out = rows.iter().map(|r| r.message(lang)).collect::<Vec<_>>().join("\n");
                if rows.len() >= MAX_ROW_PROBLEMS {
                    out.push('\n');
                    out.push_str(lang.t("… (weitere Probleme werden nicht angezeigt)", "… (further problems are not shown)"));
                }
                out
            }
            BackupError::Db(e) => format!("{}: {e}", lang.t("Datenbankfehler", "Database error")),
            BackupError::TooLarge { max_mb } => match lang {
                Lang::De => format!("Die Datei ist zu groß (höchstens {max_mb} MB)."),
                Lang::En => format!("The file is too large (at most {max_mb} MB)."),
            },
            BackupError::Cancelled => s("Der Import wurde abgebrochen – es wurde nichts angelegt.", "The import was cancelled – nothing was created."),
        }
    }
}

/// Was beim Import bewusst nicht (oder anders) übernommen wurde.
#[derive(Debug, Clone, PartialEq)]
pub enum Warning {
    /// Das Projekt hat ein neues Kürzel; Tickets wurden von 1 an neu nummeriert, Schlüssel in Texten bleiben unverändert.
    Renumbered { from_key: String, to_key: String },
    /// Ein Verknüpfungstyp fehlte hier und wurde angelegt (Name in beiden Sprachen).
    LinkTypeCreated { de: String, en: String },
    /// Verknüpfungen zu Tickets anderer Projekte wurden nicht übernommen.
    ExternalLinks(usize),
    /// Verknüpfungen mit Terminal-Sitzungen: gespeichert als Text, die Sitzungen gibt es hier nicht.
    TerminalLinks(usize),
    /// Werte von Version-/Meilenstein-Feldern: nicht Teil dieser Format-Version.
    VersionValues(usize),
    /// Ein Verweis eines Boards oder Filters konnte nicht aufgelöst werden.
    View(Unresolved),
}

impl Warning {
    pub fn message(&self, lang: Lang) -> String {
        match (self, lang) {
            (Warning::Renumbered { from_key, to_key }, Lang::De) => {
                format!("Neues Kürzel {to_key} statt {from_key}: Die Tickets wurden von 1 an neu nummeriert. Ticketschlüssel in Texten (z. B. {from_key}-12 in Kommentaren) wurden nicht umgeschrieben.")
            }
            (Warning::Renumbered { from_key, to_key }, Lang::En) => {
                format!("New key {to_key} instead of {from_key}: the tickets were renumbered from 1. Ticket keys inside texts (e.g. {from_key}-12 in comments) were not rewritten.")
            }
            (Warning::LinkTypeCreated { de, .. }, Lang::De) => format!("Verknüpfungstyp {} angelegt (gab es hier nicht).", lang.q(de)),
            (Warning::LinkTypeCreated { de: _, en }, Lang::En) => format!("Link type {} created (it did not exist here).", lang.q(en)),
            (Warning::ExternalLinks(n), Lang::De) => format!("{n} Verknüpfung(en) zu Tickets anderer Projekte nicht übernommen."),
            (Warning::ExternalLinks(n), Lang::En) => format!("{n} link(s) to tickets of other projects were not imported."),
            (Warning::TerminalLinks(n), Lang::De) => format!("{n} Terminal-Verknüpfung(en) als Text übernommen – diese Sitzungen gibt es hier nicht, sie erscheinen als beendet."),
            (Warning::TerminalLinks(n), Lang::En) => format!("{n} terminal link(s) imported as text – these sessions do not exist here and show as ended."),
            (Warning::VersionValues(n), Lang::De) => format!("{n} Wert(e) von Version-/Meilenstein-Feldern nicht übernommen (nicht Teil dieses Formats)."),
            (Warning::VersionValues(n), Lang::En) => format!("{n} value(s) of version/milestone fields were not imported (not part of this format)."),
            (Warning::View(u), l) => u.message(l),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_message_has_both_languages() {
        let t = || TicketError::TitleEmpty;
        let all = vec![
            BackupError::Format,
            BackupError::Version { found: 2, supported: 1 },
            BackupError::Json("x".into()),
            BackupError::KeyTaken("PAN".into()),
            BackupError::Plan(PlanError::NotFound),
            BackupError::Ticket(t()),
            BackupError::View(ViewError::ColumnEmpty),
            BackupError::At(Where::Ticket(3), Problem::FieldNotInType),
            BackupError::AtPlan(Where::Structure, PlanError::KeyInvalid),
            BackupError::AtTicket(Where::Comment(1, 0), t()),
            BackupError::AtView(Where::Views, ViewError::ColumnEmpty),
            BackupError::AtSprint(Where::Sprint(1), SprintError::SprintsDisabled),
            BackupError::CsvHeader(HeaderProblem::Fixed { column: 1, expected: "Key".into(), found: "x".into() }),
            BackupError::CsvHeader(HeaderProblem::Unknown { column: 10, name: "x".into() }),
            BackupError::CsvHeader(HeaderProblem::Duplicate { column: 11, name: "x".into() }),
            BackupError::CsvRows(vec![RowProblem { row: 2, column: "Title".into(), issue: RowIssue::TypeMissing }]),
            BackupError::Db("x".into()),
            BackupError::TooLarge { max_mb: 128 },
        ];
        for e in all {
            let (de, en) = (e.message(Lang::De), e.message(Lang::En));
            assert!(!de.is_empty() && !en.is_empty(), "{e:?}");
            assert_ne!(de, en, "{e:?}: deutsch und englisch sollten sich unterscheiden");
        }
    }

    #[test]
    fn problems_where_and_issues_are_bilingual() {
        for p in [
            Problem::TicketNumber(1),
            Problem::Rank(1),
            Problem::StructureNames,
            Problem::StructureMismatch,
            Problem::UnknownRef(RefKind::Status, 3),
            Problem::UnknownRef(RefKind::Ticket, 3),
            Problem::FieldNotInType,
            Problem::StatusNotAllowed,
            Problem::Timestamp("x".into()),
            Problem::Source("x".into()),
            Problem::TargetKind("x".into()),
            Problem::LinkTypeDiffers,
            Problem::LinkTypeUnnamed,
            Problem::LinkDuplicate,
            Problem::Minutes,
            Problem::SprintState("x".into()),
            Problem::SecondActiveSprint,
            Problem::TicketInTwoSprints(3),
            Problem::SprintTicketTwice(3),
            Problem::RemovedBeforeAdded,
            Problem::Points,
            Problem::UnknownRef(RefKind::Version, 1),
            Problem::UnknownRef(RefKind::Milestone, 1),
            Problem::UnknownRef(RefKind::Sprint, 1),
        ] {
            assert_ne!(p.message(Lang::De), p.message(Lang::En), "{p:?}");
        }
        for w in [Where::Project, Where::Structure, Where::Views, Where::LinkType(1), Where::Value(2, 3), Where::Comment(1, 0), Where::History(1, 0), Where::Link(1, 0), Where::Time(1, 0), Where::Milestone(1), Where::SprintEntry(1, 0)] {
            assert_ne!(w.text(Lang::De), w.text(Lang::En), "{w:?}");
        }
        assert_eq!(Where::Ticket(4).text(Lang::En), Where::Ticket(4).text(Lang::De), "„Ticket“ ist in beiden Sprachen gleich");
        assert_eq!(Where::Version(2).text(Lang::En), "Version 2");
        assert_eq!(Where::Sprint(2).text(Lang::De), "Sprint 2");
        for i in [
            RowIssue::KeyInvalid("x".into()),
            RowIssue::KeyDuplicate("x".into()),
            RowIssue::TypeMissing,
            RowIssue::TypeUnknown("x".into()),
            RowIssue::StatusUnknown("x".into()),
            RowIssue::StatusNotAllowed("x".into()),
            RowIssue::BoolInvalid("x".into()),
            RowIssue::TimestampInvalid("x".into()),
            RowIssue::ParentUnknown("x".into()),
            RowIssue::ParentCycle,
            RowIssue::CellInvalid("x".into()),
            RowIssue::OptionUnknown("x".into()),
            RowIssue::FieldNotInType("x".into()),
            RowIssue::TicketRefUnknown("x".into()),
            RowIssue::Ticket(TicketError::TitleEmpty),
            RowIssue::ReleaseUnknown("x".into()),
            RowIssue::SprintUnknown("x".into()),
            RowIssue::Sprint(SprintError::SprintCompleted),
        ] {
            assert_ne!(i.message(Lang::De), i.message(Lang::En), "{i:?}");
        }
        let w = Warning::Renumbered { from_key: "AA".into(), to_key: "BB".into() };
        assert!(w.message(Lang::En).contains("BB") && w.message(Lang::De).contains("AA"));
        for w in [
            Warning::LinkTypeCreated { de: "a".into(), en: "b".into() },
            Warning::ExternalLinks(2),
            Warning::TerminalLinks(1),
            Warning::VersionValues(4),
        ] {
            assert_ne!(w.message(Lang::De), w.message(Lang::En), "{w:?}");
        }
    }

    #[test]
    fn row_problem_names_row_and_column() {
        let p = RowProblem { row: 7, column: "Status".into(), issue: RowIssue::StatusUnknown("Zzz".into()) };
        assert!(p.message(Lang::En).starts_with("Row 7, column “Status”"));
        assert!(p.message(Lang::De).starts_with("Zeile 7, Spalte „Status“"));
        let q = RowProblem { row: 3, column: String::new(), issue: RowIssue::TypeMissing };
        assert!(q.message(Lang::En).starts_with("Row 3:"));
    }
}
