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
//!   Was das Format (noch) nicht abbildet, wird übersprungen und im Bericht gemeldet ([`Warning`]): Verknüpfungen zu
//!   Tickets anderer Projekte, Werte von Version-/Meilenstein-Feldern. Terminal-Verknüpfungen bleiben als Text erhalten.
//! * Der Ticket-Zähler (`next_number`) wird fortgesetzt (nie kleiner als höchste Nummer + 1).
//!
//! # Erweiterbarkeit
//!
//! Das Dokument ist für weitere Abschnitte gebaut (Sprints, Versionen, Meilensteine): neue Abschnitte werden als
//! `#[serde(default)]`-Felder von [`ProjectDoc`] ergänzt, ihre Verweise benutzen dieselben Nummern, und die
//! [`VERSION`] wird erhöht – ältere Programmversionen lehnen ein neueres Dokument ab (statt Teile zu verlieren). Ein Dokument
//! mit niedrigerer Version bleibt lesbar, weil fehlende Abschnitte leer sind.
//!
//! # Tickets als CSV
//!
//! Siehe [`tickets_csv`].

pub mod error;
mod export;
mod import;
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
pub use import::{check_new_key, import_project, ImportReport, ImportResult};

use crate::planning::projects::TagColorMode;
use crate::planning::template::TemplateDoc;
use crate::planning::views::transfer::ViewsDoc;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Kennung des Dokumentformats.
pub const FORMAT: &str = "paneldeck-project";
/// Aktuelle Format-Version.
pub const VERSION: u32 = 1;

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
    pub comments: usize,
    pub links: usize,
    pub time_entries: usize,
    pub history: usize,
}

/// `JJJJ-MM-TTTHH:MM:SS` – das Zeitformat der Datenbank.
pub(crate) fn valid_timestamp(s: &str) -> bool {
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").is_ok()
}
