//! Export und Import: vollständige Projektsicherung (JSON) und Tickets als CSV. Reine Datenlogik auf `&Connection`.
//!
//! # Projektsicherung (`paneldeck-project`)
//!
//! * [`export_project`] liefert ein [`ProjectDoc`]: Projekt (alle Einstellungen), die Struktur als [`TemplateDoc`]
//!   (Status, Typen, Hierarchie, Felder, Workflows und Übergangsregeln, Quick-Actions, Arten der Arbeit), Boards und
//!   gespeicherte Filter ([`ViewsDoc`]), die benutzten Verknüpfungstypen und alle Tickets mit Feldwerten, Kommentaren,
//!   Verlauf, Verknüpfungen und Zeiteinträgen.
//! * **Kennungen im Dokument sind keine Datenbank-Ids:** Status, Typen und Felder tragen die Vorlagen-Nummern der
//!   Struktur, Arten der Arbeit ihre Position (1, 2, …), Verknüpfungstypen eine Nummer in `link_types`, Tickets ihre
//!   Ticketnummer im Projekt. Auch die Kennungen im Verlauf (`old`/`new`, `field:<n>`, Verknüpfungs- und Zeit-JSON) sind
//!   so übersetzt; die Aktions-Nummern (`batch`) sind von 1 an dicht durchgezählt. Zwei Exporte desselben Stands sind
//!   daher bis auf `exported_at` gleich, und Export → Import → Export ergibt wieder dasselbe Dokument.
//! * [`import_project`] legt **immer ein neues Projekt** an und ändert nie Bestehendes. Ist das Kürzel vergeben, liefert er
//!   [`BackupError::KeyTaken`]; der Aufrufer fragt ein neues Kürzel ab und ruft erneut auf. Mit einem anderen Kürzel als
//!   dem des Dokuments werden die Tickets von 1 an **umnummeriert** (Reihenfolge der alten Nummern); die interne
//!   Verknüpfung (Eltern, Ticket-Felder, Verknüpfungen, Verlauf) bleibt korrekt, Schlüssel *in Texten* (Titel,
//!   Kommentare, Verlaufstexte) werden nicht umgeschrieben (Hinweis im Bericht).
//! * Der Import ist atomar (ein SAVEPOINT) und prüft jedes Element mit den Regeln der Datenschicht: Namen, Farben,
//!   Optionen, Workflows und Hierarchie über `create_project`/`import_types`/`import_views`, Feldwerte über
//!   `values::normalize_value`, Eltern über `tickets::check_parent`, Verknüpfungsziele über `links::clean_target`,
//!   Status gegen den Workflow des Typs. **Verletzt ein Element eine Regel, bricht der Import mit einem Fehler samt
//!   Fundstelle ab** (z. B. „Ticket 12, Feld 3: …“). Nicht geprüft werden Pflichtfelder: Ein Ticket, das nach dem
//!   Anlegen eines Pflichtfelds noch leer ist, gehört zum Bestand und wird so wiederhergestellt.
//!   Was das Format nicht abbildet, wird übersprungen und im Bericht gemeldet ([`Warning`]): Verknüpfungen zu
//!   Tickets anderer Projekte (und bei Version-1-Dokumenten Version-/Meilenstein-Werte). Terminal-Verknüpfungen bleiben
//!   als Text erhalten.
//! * Der Ticket-Zähler (`next_number`) wird fortgesetzt (nie kleiner als höchste Nummer + 1).
//!
//! # Sprints, Versionen, Meilensteine (ab Format-Version 2)
//!
//! * Die optionalen Abschnitte `versions`, `milestones` und `sprints` (letzterer mit den Zeilen der Sprint-Zugehörigkeit,
//!   `tickets` mit `added_at`, `removed_at`, `points_at_add`) stehen im Dokument unabhängig von den Projektschaltern
//!   `use_sprints`/`use_versions`: Die Daten bleiben auch bei ausgeschalteter Funktion erhalten und werden so gesichert.
//!   Versionen, Meilensteine und Sprints tragen Nummern (1, 2, … in der Reihenfolge der Konfiguration bzw. des Anlegens);
//!   Feldwerte vom Typ Version/Meilenstein und der Verlauf dieser Felder nennen diese Nummern.
//! * Reihenfolge beim Import: Projekt → Struktur → Versionen/Meilensteine → Sprints → Tickets samt Feldwerten → Zeilen der
//!   Sprint-Zugehörigkeit. Namen, Termine, Zustände (höchstens ein aktiver Sprint) und „ein Ticket in höchstens einem offenen
//!   Sprint“ werden mit den Regeln der Sprint-Schicht geprüft; Burndown und Bericht ergeben nach dem Import dasselbe wie vorher.
//! * Ein Dokument der **Version 1** bleibt importierbar (die Abschnitte fehlen dann); seine Version-/Meilenstein-Werte sind
//!   Datenbank-Ids ohne Objekte und werden wie bisher übersprungen und gemeldet ([`Warning::VersionValues`]).
//!
//! # Große Dateien: Fortschritt und Abbruch
//!
//! [`import_project_with`] und [`tickets_csv::import_tickets_csv_with`] melden den Fortschritt an einen [`Progress`] und brechen ab,
//! wenn er es verlangt ([`BackupError::Cancelled`]); weil beides in einem SAVEPOINT läuft, entsteht dann nichts. Die Oberfläche
//! benutzt das, um den Import in einem Hintergrund-Faden auf einer eigenen Verbindung laufen zu lassen (`planning_ui/backup_run.rs`).
//! [`import_project`] und [`tickets_csv::import_tickets_csv`] sind dieselben Läufe ohne Beobachter.
//!
//! # Erweiterbarkeit
//!
//! Neue Abschnitte werden als `#[serde(default)]`-Felder von [`ProjectDoc`] ergänzt, ihre Verweise benutzen dieselben
//! Nummern, und die [`VERSION`] wird erhöht – ältere Programmversionen lehnen ein neueres Dokument ab (statt Teile zu
//! verlieren). Ein Dokument mit niedrigerer Version bleibt lesbar, weil fehlende Abschnitte leer sind.
//!
//! # Tickets als CSV
//!
//! Siehe [`tickets_csv`].

pub mod error;
mod export;
mod import;
pub mod progress;
mod refmap;
pub mod tickets_csv;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_csv;

#[allow(unused_imports)]
pub use error::{BackupError, HeaderProblem, Problem, RefKind, RowIssue, RowProblem, Warning, Where};
pub use export::export_project;
#[allow(unused_imports)]
pub use import::{check_new_key, import_project, import_project_with, ImportReport, ImportResult, MAX_RANK, MAX_TICKET_NUMBER};
pub use progress::Progress;

use crate::planning::projects::TagColorMode;
use crate::planning::template::TemplateDoc;
use crate::planning::views::transfer::ViewsDoc;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Größte Datei (JSON-Sicherung oder Ticket-CSV), die zum Import gelesen wird. Eine größere Datei ist fast sicher kein Projekt
/// und würde den Arbeitsspeicher füllen, bevor irgendetwas geprüft ist.
pub const MAX_FILE_BYTES: u64 = 128 * 1024 * 1024;

/// Liest eine Importdatei höchstens bis [`MAX_FILE_BYTES`]; eine größere Datei ergibt `BackupError::TooLarge` (der Inhalt wird
/// dann nicht gelesen). Der äußere Fehler ist ein Lesefehler des Dateisystems.
pub fn read_limited(path: &std::path::Path) -> std::io::Result<Result<Vec<u8>, BackupError>> {
    use std::io::Read;
    let too_large = || BackupError::TooLarge { max_mb: MAX_FILE_BYTES / (1024 * 1024) };
    let file = std::fs::File::open(path)?;
    if file.metadata().map(|m| m.len() > MAX_FILE_BYTES).unwrap_or(false) {
        return Ok(Err(too_large()));
    }
    // auch lesen, wenn die Länge nicht stimmt (Pipes, wachsende Dateien): eine Grenze mehr als erlaubt, dann ist sie überschritten
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Ok(Err(too_large()));
    }
    Ok(Ok(bytes))
}

/// Kennung des Dokumentformats.
pub const FORMAT: &str = "paneldeck-project";
/// Aktuelle Format-Version.
pub const VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectDoc {
    pub format: String,
    pub version: u32,
    /// Zeitpunkt des Exports (lokale Zeit, ISO).
    #[serde(default)]
    pub exported_at: String,
    /// Version von Paneldeck, die exportiert hat.
    #[serde(default)]
    pub app_version: String,
    pub project: DocProject,
    /// Status, Typen, Hierarchie, Felder, Workflows, Quick-Actions und Arten der Arbeit.
    pub structure: TemplateDoc,
    /// Die von Tickets benutzten Verknüpfungstypen (Nummern wie in den Verknüpfungen und im Verlauf).
    #[serde(default)]
    pub link_types: Vec<DocLinkType>,
    /// Boards und gespeicherte Filter des Projekts.
    pub views: ViewsDoc,
    /// Alle Tickets, aufsteigend nach Nummer.
    #[serde(default)]
    pub tickets: Vec<DocTicket>,
    /// Versionen in der Reihenfolge der Konfiguration (Nummer = Position, von 1 an). Ab Version 2.
    #[serde(default)]
    pub versions: Vec<DocRelease>,
    /// Meilensteine (wie Versionen). Ab Version 2.
    #[serde(default)]
    pub milestones: Vec<DocRelease>,
    /// Sprints in der Reihenfolge des Anlegens (Nummer = Position, von 1 an). Ab Version 2.
    #[serde(default)]
    pub sprints: Vec<DocSprint>,
}

/// Eine Version oder ein Meilenstein.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocRelease {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Fälligkeit `JJJJ-MM-TT`.
    #[serde(default)]
    pub due_date: Option<String>,
    /// Erschienen (Version) bzw. erreicht (Meilenstein).
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub done_at: Option<String>,
}

/// Ein Sprint samt der Zeilen seiner Ticket-Zugehörigkeit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocSprint {
    pub name: String,
    #[serde(default)]
    pub goal: String,
    #[serde(default)]
    pub start_date: Option<String>,
    #[serde(default)]
    pub end_date: Option<String>,
    /// `planned`, `active` oder `completed`.
    pub state: String,
    /// Dauer-Vorlage in Arbeitstagen.
    pub default_days: u32,
    pub created_at: String,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub completed_at: Option<String>,
    /// Zugehörigkeit der Tickets (auch frühere, mit `removed_at`), in der Reihenfolge der Datenbank.
    #[serde(default)]
    pub tickets: Vec<DocSprintTicket>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocSprintTicket {
    /// Ticketnummer.
    pub ticket: i64,
    pub added_at: String,
    #[serde(default)]
    pub removed_at: Option<String>,
    #[serde(default)]
    pub points_at_add: Option<f64>,
}

/// Die Einstellungen des Projekts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocProject {
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub color: String,
    #[serde(default)]
    pub work_dir: String,
    #[serde(default)]
    pub use_sprints: bool,
    #[serde(default)]
    pub use_versions: bool,
    pub tag_color_mode: TagColorMode,
    /// Nummer des nächsten Tickets (wird nie zurückgesetzt).
    pub next_number: i64,
    pub created_at: String,
    /// Schwellen der Fälligkeits-Markierung (ab Paneldeck 2.3). Fehlt das Feld (ältere Sicherung), bekommt ein Projekt mit den
    /// mitgelieferten Typen die Standard-Schwellen, sonst keine – wie beim Update der Datenbank.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_thresholds: Option<Vec<crate::due::DueThreshold>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocLinkType {
    pub id: i64,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub name_out: Option<String>,
    #[serde(default)]
    pub name_in: Option<String>,
    #[serde(default)]
    pub symmetric: bool,
    /// `ticket`, `terminal`, `url` oder `branch`.
    pub target_kind: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocTicket {
    pub number: i64,
    /// Vorlagen-Nummer des Typs.
    #[serde(rename = "type")]
    pub type_id: i64,
    pub title: String,
    /// Vorlagen-Nummer des Status (leer bei Typen ohne Workflow).
    #[serde(default)]
    pub status: Option<i64>,
    /// Ticketnummer des Eltern-Tickets.
    #[serde(default)]
    pub parent: Option<i64>,
    #[serde(default)]
    pub rank: i64,
    #[serde(default)]
    pub flagged: bool,
    #[serde(default)]
    pub starred: bool,
    pub created_at: String,
    pub updated_at: String,
    /// Gesetzte Feldwerte nach Feld-Nummer; Ticket-Felder enthalten die Ticketnummer des Ziels.
    #[serde(default)]
    pub values: Vec<DocValue>,
    #[serde(default)]
    pub comments: Vec<DocComment>,
    #[serde(default)]
    pub history: Vec<DocHistory>,
    #[serde(default)]
    pub links: Vec<DocLink>,
    #[serde(default)]
    pub time_entries: Vec<DocTime>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocValue {
    /// Vorlagen-Nummer des Felds.
    pub field: i64,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocComment {
    pub body: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocHistory {
    pub at: String,
    /// `ui`, `quick`, `agent` oder `import`.
    pub source: String,
    #[serde(default)]
    pub source_detail: Option<String>,
    /// Art der Änderung (`status`, `title`, `field:<Feld-Nummer>` …).
    pub field: String,
    #[serde(default)]
    pub old: Option<String>,
    #[serde(default)]
    pub new: Option<String>,
    /// Gleiche Nummer = eine Aktion (von 1 an dicht durchgezählt).
    pub batch: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocLink {
    /// Nummer in `link_types`.
    pub link_type: i64,
    pub target: DocTarget,
    pub created_at: String,
}

/// Ziel einer Verknüpfung.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DocTarget {
    /// Ticket im selben Projekt (Ticketnummer).
    Ticket { number: i64 },
    /// Ticket eines anderen Projekts (Schlüssel wie `OTH-3`); wird beim Import nicht übernommen.
    ExternalTicket { key: String },
    /// Kennung einer Terminal-Sitzung (nur als Text; die Sitzung gibt es anderswo nicht).
    Terminal { text: String },
    Url { text: String },
    Branch { repo: String, name: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocTime {
    pub start: String,
    pub minutes: u32,
    /// Position der Art der Arbeit (1-basiert) in `structure.work_kinds`.
    #[serde(default)]
    pub work_kind: Option<i64>,
    #[serde(default)]
    pub comment: String,
    pub created_at: String,
}

impl ProjectDoc {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Liest ein Dokument und prüft Format und Version.
    pub fn from_json(text: &str) -> Result<ProjectDoc, BackupError> {
        let text = text.trim_start_matches('\u{feff}');
        let v: Value = serde_json::from_str(text).map_err(|e| BackupError::Json(e.to_string()))?;
        if v.get("format").and_then(Value::as_str) != Some(FORMAT) {
            return Err(BackupError::Format);
        }
        let found = v.get("version").and_then(Value::as_u64).unwrap_or(0) as u32;
        if found > VERSION {
            return Err(BackupError::Version { found, supported: VERSION });
        }
        serde_json::from_value(v).map_err(|e| BackupError::Json(e.to_string()))
    }

    /// Übersicht für die Vorschau vor dem Import.
    pub fn preview(&self) -> Preview {
        Preview {
            key: self.project.key.clone(),
            name: self.project.name.clone(),
            exported_at: self.exported_at.clone(),
            app_version: self.app_version.clone(),
            statuses: self.structure.statuses.len(),
            types: self.structure.types.len(),
            fields: self.structure.types.iter().map(|t| t.fields.len()).sum(),
            boards: self.views.boards.len(),
            filters: self.views.filters.len(),
            tickets: self.tickets.len(),
            sprints: self.sprints.len(),
            versions: self.versions.len(),
            milestones: self.milestones.len(),
            comments: self.tickets.iter().map(|t| t.comments.len()).sum(),
            links: self.tickets.iter().map(|t| t.links.len()).sum(),
            time_entries: self.tickets.iter().map(|t| t.time_entries.len()).sum(),
            history: self.tickets.iter().map(|t| t.history.len()).sum(),
        }
    }
}

/// Was eine Sicherung enthält (für die Vorschau).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Preview {
    pub key: String,
    pub name: String,
    pub exported_at: String,
    pub app_version: String,
    pub statuses: usize,
    pub types: usize,
    pub fields: usize,
    pub boards: usize,
    pub filters: usize,
    pub tickets: usize,
    pub sprints: usize,
    pub versions: usize,
    pub milestones: usize,
    pub comments: usize,
    pub links: usize,
    pub time_entries: usize,
    pub history: usize,
}

/// `JJJJ-MM-TTTHH:MM:SS` – das Zeitformat der Datenbank.
pub(crate) fn valid_timestamp(s: &str) -> bool {
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").is_ok()
}
