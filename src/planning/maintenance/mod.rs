//! Wartung der Experten-Projekte (Issue #308): Prüfungen der Tickets, der Konfiguration und der Datenbank, eigene Prüfregeln,
//! gezielte Aktionen mit Vorschau, Datenbankpflege (Prüfen, Verdichten, Statistik, Reparatur, Rettung) und das Wartungsprotokoll.
//! Reine Datenlogik ohne Oberfläche (die Seite steht in `app/planning_ui/maint_*.rs`).
//!
//! # Regeln (Entscheidungen des Interviews vom 2026-11-20)
//!
//! * Ein **Bericht** plus gezielte **Aktionen**; nichts passiert automatisch. Jede Aktion zeigt vorher eine Vorschau und legt
//!   eine Sicherung „maintenance“ an (höchstens drei, `backup::Kind::PreMaintenance`), jede Aktion steht im Protokoll.
//! * Die Wartung **löscht nie Tickets**: Sie archiviert, wechselt den Status, verknüpft. Entfernt wird nur ungenutzte
//!   Konfiguration; Benutztes lässt sich nur zusammenführen (Labels).
//! * „Als Won't Do schließen“ setzt den je Projekt gewählten Zielstatus ([`Settings::wont_do`]); ohne ihn ist die Aktion aus.
//! * Befunde haben eine **stabile Kennung** ([`Finding::id`]); ignorierte Befunde stehen in `maintenance_ignored`.
//! * Lesende Prüfungen sind an (abschaltbar je Projekt), eigene Regeln aus, bis jemand eine anlegt.
//! * Während eine Aktion schreibt, ist die Wartung „beschäftigt“ ([`busy`]): Schreibende MCP-Werkzeuge antworten dann mit dem
//!   Code `busy` und `retry_after_ms`.

pub mod actions;
pub mod checks_config;
pub mod checks_tickets;
pub mod dbcare;
pub mod rules;
pub mod schema;
pub mod similar;

#[cfg(test)]
pub(crate) mod fixture;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_db;

use crate::state::Lang;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};

/// Höchstzahl Befunde, die `maintenance_report` (MCP) liefert.
pub const MCP_MAX_FINDINGS: usize = 200;
/// So lange sollen Agenten warten, bevor sie es nach `busy` erneut versuchen.
pub const RETRY_AFTER_MS: u64 = 2000;

// ------------------------------------------------------------------ Sperre während schreibender Aktionen

/// Ende der Sperre in Millisekunden seit dem Start (0 = frei). Wird von der laufenden Aktion gesetzt und am Ende gelöscht.
static LOCK_DEPTH: AtomicU64 = AtomicU64::new(0);

/// Schreibsperre der Wartung (RAII): solange sie lebt, gilt [`busy`].
pub struct WriteLock(());

impl WriteLock {
    pub fn take() -> WriteLock {
        LOCK_DEPTH.fetch_add(1, Ordering::SeqCst);
        WriteLock(())
    }
}

impl Drop for WriteLock {
    fn drop(&mut self) {
        LOCK_DEPTH.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Nur für den Ende-zu-Ende-Test (`tools/smoke-mcp.ps1`): `PANELDECK_TEST_MAINT_BUSY=1` hält die Sperre dauerhaft, damit der
/// Fehler `busy` ohne Mausklick prüfbar ist. Ändert keine Daten.
fn test_hold() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("PANELDECK_TEST_MAINT_BUSY").as_deref() == Ok("1"))
}

/// Schreibt gerade eine Wartungsaktion? Dann die empfohlene Wartezeit in Millisekunden.
pub fn busy() -> Option<u64> {
    (LOCK_DEPTH.load(Ordering::SeqCst) > 0 || test_hold()).then_some(RETRY_AFTER_MS)
}

// ------------------------------------------------------------------ Einordnung der Befunde

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    Tickets,
    Configuration,
    Database,
    Rules,
}

impl Category {
    pub const ALL: [Category; 4] = [Category::Tickets, Category::Configuration, Category::Database, Category::Rules];

    pub fn as_str(self) -> &'static str {
        match self {
            Category::Tickets => "tickets",
            Category::Configuration => "configuration",
            Category::Database => "database",
            Category::Rules => "rules",
        }
    }

    pub fn parse(s: &str) -> Option<Category> {
        Category::ALL.into_iter().find(|c| c.as_str() == s)
    }

    pub fn label(self, lang: Lang) -> &'static str {
        match self {
            Category::Tickets => lang.t("Tickets", "Tickets"),
            Category::Configuration => lang.t("Konfiguration", "Configuration"),
            Category::Database => lang.t("Datenbank", "Database"),
            Category::Rules => lang.t("Eigene Regeln", "Custom rules"),
        }
    }
}

/// Schwere: Fehler (kaputte Struktur, Datenbank), Warnung (überfällig, liegengeblieben), Hinweis (ungenutzte Konfiguration,
/// mögliche Duplikate).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Hint,
}

impl Severity {
    pub const ALL: [Severity; 3] = [Severity::Error, Severity::Warning, Severity::Hint];

    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Hint => "hint",
        }
    }

    pub fn parse(s: &str) -> Option<Severity> {
        Severity::ALL.into_iter().find(|x| x.as_str() == s)
    }

    pub fn label(self, lang: Lang) -> &'static str {
        match self {
            Severity::Error => lang.t("Fehler", "Error"),
            Severity::Warning => lang.t("Warnung", "Warning"),
            Severity::Hint => lang.t("Hinweis", "Hint"),
        }
    }
}

/// Die Prüfungen. Die Kennung ([`Check::as_str`]) ist Teil der öffentlichen MCP-Antwort (`kind`) und der gespeicherten
/// Einstellungen (abgeschaltete Prüfungen) – nie umbenennen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Check {
    // Tickets
    StaleInProgress,
    StaleNew,
    Overdue,
    OpenInFinished,
    OrphanedSubtask,
    BrokenReference,
    ParentDoneChildrenOpen,
    LinkCycle,
    MissingRequired,
    InvalidValue,
    PossibleDuplicate,
    // Konfiguration
    FieldUnused,
    FieldStale,
    OptionUnused,
    TypeUnused,
    StatusUnused,
    StatusUnreachable,
    LabelsSimilar,
    LinkTypeUnused,
    WorkKindUnused,
    QuickActionBroken,
    DashboardBroken,
    FilterBroken,
    BoardBroken,
    // Datenbank
    DbIntegrity,
    DbForeignKey,
    DbCounter,
    DbHistory,
    // Eigene Regeln
    CustomRule,
}

impl Check {
    pub const ALL: [Check; 29] = [
        Check::StaleInProgress,
        Check::StaleNew,
        Check::Overdue,
        Check::OpenInFinished,
        Check::OrphanedSubtask,
        Check::BrokenReference,
        Check::ParentDoneChildrenOpen,
        Check::LinkCycle,
        Check::MissingRequired,
        Check::InvalidValue,
        Check::PossibleDuplicate,
        Check::FieldUnused,
        Check::FieldStale,
        Check::OptionUnused,
        Check::TypeUnused,
        Check::StatusUnused,
        Check::StatusUnreachable,
        Check::LabelsSimilar,
        Check::LinkTypeUnused,
        Check::WorkKindUnused,
        Check::QuickActionBroken,
        Check::DashboardBroken,
        Check::FilterBroken,
        Check::BoardBroken,
        Check::DbIntegrity,
        Check::DbForeignKey,
        Check::DbCounter,
        Check::DbHistory,
        Check::CustomRule,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Check::StaleInProgress => "stale_in_progress",
            Check::StaleNew => "stale_new",
            Check::Overdue => "overdue",
            Check::OpenInFinished => "open_in_finished",
            Check::OrphanedSubtask => "orphaned_subtask",
            Check::BrokenReference => "broken_reference",
            Check::ParentDoneChildrenOpen => "parent_done_children_open",
            Check::LinkCycle => "link_cycle",
            Check::MissingRequired => "missing_required",
            Check::InvalidValue => "invalid_value",
            Check::PossibleDuplicate => "possible_duplicate",
            Check::FieldUnused => "field_unused",
            Check::FieldStale => "field_stale",
            Check::OptionUnused => "option_unused",
            Check::TypeUnused => "type_unused",
            Check::StatusUnused => "status_unused",
            Check::StatusUnreachable => "status_unreachable",
            Check::LabelsSimilar => "labels_similar",
            Check::LinkTypeUnused => "link_type_unused",
            Check::WorkKindUnused => "work_kind_unused",
            Check::QuickActionBroken => "quick_action_broken",
            Check::DashboardBroken => "dashboard_broken",
            Check::FilterBroken => "filter_broken",
            Check::BoardBroken => "board_broken",
            Check::DbIntegrity => "db_integrity",
            Check::DbForeignKey => "db_foreign_key",
            Check::DbCounter => "db_counter",
            Check::DbHistory => "db_history",
            Check::CustomRule => "custom_rule",
        }
    }

    pub fn parse(s: &str) -> Option<Check> {
        Check::ALL.into_iter().find(|c| c.as_str() == s)
    }

    pub fn category(self) -> Category {
        use Check::*;
        match self {
            StaleInProgress | StaleNew | Overdue | OpenInFinished | OrphanedSubtask | BrokenReference | ParentDoneChildrenOpen | LinkCycle | MissingRequired | InvalidValue | PossibleDuplicate => Category::Tickets,
            FieldUnused | FieldStale | OptionUnused | TypeUnused | StatusUnused | StatusUnreachable | LabelsSimilar | LinkTypeUnused | WorkKindUnused | QuickActionBroken | DashboardBroken | FilterBroken | BoardBroken => Category::Configuration,
            DbIntegrity | DbForeignKey | DbCounter | DbHistory => Category::Database,
            CustomRule => Category::Rules,
        }
    }

    /// Standard-Schwere (eigene Regeln bringen ihre eigene mit).
    pub fn severity(self) -> Severity {
        use Check::*;
        match self {
            OrphanedSubtask | BrokenReference | ParentDoneChildrenOpen | LinkCycle | MissingRequired | InvalidValue | DbIntegrity | DbForeignKey | DbCounter | DbHistory => Severity::Error,
            StaleInProgress | StaleNew | Overdue | OpenInFinished | StatusUnreachable | QuickActionBroken | DashboardBroken | FilterBroken | BoardBroken | CustomRule => Severity::Warning,
            PossibleDuplicate | FieldUnused | FieldStale | OptionUnused | TypeUnused | StatusUnused | LabelsSimilar | LinkTypeUnused | WorkKindUnused => Severity::Hint,
        }
    }

    /// Lässt sich die Prüfung je Projekt abschalten? (Datenbank-Prüfungen und eigene Regeln nicht: die einen sind Pflicht,
    /// die anderen haben einen eigenen Schalter je Regel.)
    pub fn switchable(self) -> bool {
        !matches!(self.category(), Category::Database | Category::Rules)
    }

    /// Kurzer Name für Einstellungen und Filter.
    pub fn label(self, lang: Lang) -> &'static str {
        use Check::*;
        match self {
            StaleInProgress => lang.t("Liegengeblieben (in Arbeit)", "Stale (in progress)"),
            StaleNew => lang.t("Liegengeblieben (nie begonnen)", "Stale (never started)"),
            Overdue => lang.t("Überfällig", "Overdue"),
            OpenInFinished => lang.t("Offen in abgeschlossenem Sprint/Release", "Open in finished sprint/release"),
            OrphanedSubtask => lang.t("Verwaiste Unteraufgaben", "Orphaned subtasks"),
            BrokenReference => lang.t("Verweise auf gelöschte Tickets", "References to deleted tickets"),
            ParentDoneChildrenOpen => lang.t("Eltern erledigt, Kinder offen", "Parent done, children open"),
            LinkCycle => lang.t("Zyklen in Verknüpfungen", "Link cycles"),
            MissingRequired => lang.t("Fehlende Pflichtfelder", "Missing required fields"),
            InvalidValue => lang.t("Ungültige Werte", "Invalid values"),
            PossibleDuplicate => lang.t("Mögliche Duplikate", "Possible duplicates"),
            FieldUnused => lang.t("Ungenutzte Felder", "Unused fields"),
            FieldStale => lang.t("Lange nicht gesetzte Felder", "Stale fields"),
            OptionUnused => lang.t("Ungenutzte Auswahlwerte", "Unused options"),
            TypeUnused => lang.t("Typen ohne Tickets", "Types without tickets"),
            StatusUnused => lang.t("Ungenutzte Status", "Unused statuses"),
            StatusUnreachable => lang.t("Unerreichbare Status", "Unreachable statuses"),
            LabelsSimilar => lang.t("Fast gleiche Labels", "Near-identical labels"),
            LinkTypeUnused => lang.t("Ungenutzte Verknüpfungstypen", "Unused link types"),
            WorkKindUnused => lang.t("Ungenutzte Arten der Arbeit", "Unused kinds of work"),
            QuickActionBroken => lang.t("Kaputte Schnellaktionen", "Broken quick actions"),
            DashboardBroken => lang.t("Dashboards mit kaputten Abfragen", "Dashboards with broken queries"),
            FilterBroken => lang.t("Gespeicherte Filter mit Fehlern", "Saved filters with errors"),
            BoardBroken => lang.t("Boards mit gelöschten Status", "Boards with deleted statuses"),
            DbIntegrity => lang.t("Integrität der Datenbank", "Database integrity"),
            DbForeignKey => lang.t("Verwaiste Zeilen", "Orphaned rows"),
            DbCounter => lang.t("Falsche Zähler", "Wrong counters"),
            DbHistory => lang.t("Verlauf ohne Ticket", "History without ticket"),
            CustomRule => lang.t("Eigene Regel", "Custom rule"),
        }
    }
}

/// Ein Ticket in einem Befund.
#[derive(Debug, Clone, PartialEq)]
pub struct TicketRef {
    pub id: i64,
    pub key: String,
    pub title: String,
}

/// Worauf sich ein Befund bezieht (bestimmt die Aktionen).
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    None,
    Ticket,
    /// Mehrere Tickets (Zyklus, Kinder eines erledigten Eltern-Tickets).
    Tickets(Vec<i64>),
    Field(i64),
    FieldOption { field: i64, option: i64 },
    Status(i64),
    Type(i64),
    LinkType(i64),
    WorkKind(i64),
    QuickAction(i64),
    Dashboard(i64),
    Filter(i64),
    Board(i64),
    /// Fast gleiche Labels: die Schreibweise, die bleibt, und die, die in sie übergehen.
    Labels { keep: String, merge: Vec<String> },
    Rule(i64),
    Database,
}

/// Mögliche Aktion zu einem Befund.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fix {
    /// In den Zielstatus „Won't Do“ des Projekts wechseln, mit Kommentar.
    CloseWontDo,
    Archive,
    SetDue,
    /// Als Duplikat verknüpfen (optional schließen).
    LinkDuplicate,
    /// Ungenutzte Konfiguration entfernen.
    Remove,
    MergeLabels,
    /// Verweis auf ein gelöschtes Eltern-Ticket lösen.
    DetachParent,
    /// Verweis/ungültigen Wert entfernen.
    ClearReference,
    /// Vorgeschlagene Aktion der eigenen Regel.
    RuleAction,
    /// Datenbank reparieren (eindeutige Fälle).
    Repair,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    /// Stabile Kennung (z. B. `stale_new:PAN-12`, `possible_duplicate:PAN-3:PAN-9`, `field_unused:f17`) – für „ignorieren“.
    pub id: String,
    pub check: Check,
    pub severity: Severity,
    pub ticket: Option<TicketRef>,
    /// Zweites Ticket (Duplikat-Partner).
    pub other: Option<TicketRef>,
    pub target: Target,
    /// Erklärung in der Sprache des Berichts.
    pub message: String,
    pub fixes: Vec<Fix>,
    /// Zusatzangaben (Tage, Prozent, Name einer Regel …) für MCP und Vorschau.
    pub detail: Value,
}

impl Finding {
    pub fn category(&self) -> Category {
        self.check.category()
    }
}

// ------------------------------------------------------------------ Einstellungen je Projekt

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// Zielstatus für „Als Won't Do schließen“ (Kategorie egal, meist „erledigt“).
    pub wont_do: Option<i64>,
    pub stale_days: i64,
    pub field_days: i64,
    pub duplicate_percent: i64,
    /// Abgeschaltete Prüfungen.
    pub disabled: Vec<Check>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { wont_do: None, stale_days: 90, field_days: 180, duplicate_percent: 80, disabled: Vec::new() }
    }
}

impl Settings {
    pub fn enabled(&self, c: Check) -> bool {
        !self.disabled.contains(&c)
    }
}

pub fn load_settings(conn: &Connection, project_id: i64) -> rusqlite::Result<Settings> {
    let row = conn
        .query_row(
            "SELECT wont_do_status_id, stale_days, field_days, duplicate_percent, disabled_checks FROM maintenance_settings WHERE project_id = ?1",
            [project_id],
            |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, String>(4)?)),
        )
        .optional()?;
    let Some((wont_do, stale_days, field_days, duplicate_percent, disabled)) = row else { return Ok(Settings::default()) };
    let disabled: Vec<String> = serde_json::from_str(&disabled).unwrap_or_default();
    // Der Zielstatus muss zum Projekt gehören (ein von außen eingetragener fremder Status gilt als nicht gesetzt).
    let wont_do = match wont_do {
        Some(s) => conn.query_row("SELECT id FROM statuses WHERE id = ?1 AND project_id = ?2", params![s, project_id], |r| r.get(0)).optional()?,
        None => None,
    };
    Ok(Settings { wont_do, stale_days, field_days, duplicate_percent, disabled: disabled.iter().filter_map(|s| Check::parse(s)).collect() })
}

/// Grenzen der Schwellen.
pub const DAYS_RANGE: std::ops::RangeInclusive<i64> = 1..=3650;
pub const PERCENT_RANGE: std::ops::RangeInclusive<i64> = 50..=100;

pub fn save_settings(conn: &Connection, project_id: i64, s: &Settings) -> rusqlite::Result<()> {
    let clamp = |v: i64, r: &std::ops::RangeInclusive<i64>| v.clamp(*r.start(), *r.end());
    let disabled: Vec<&str> = s.disabled.iter().map(|c| c.as_str()).collect();
    conn.execute(
        "INSERT INTO maintenance_settings (project_id, wont_do_status_id, stale_days, field_days, duplicate_percent, disabled_checks) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(project_id) DO UPDATE SET wont_do_status_id = excluded.wont_do_status_id, stale_days = excluded.stale_days, field_days = excluded.field_days,
         duplicate_percent = excluded.duplicate_percent, disabled_checks = excluded.disabled_checks",
        params![project_id, s.wont_do, clamp(s.stale_days, &DAYS_RANGE), clamp(s.field_days, &DAYS_RANGE), clamp(s.duplicate_percent, &PERCENT_RANGE), serde_json::to_string(&disabled).unwrap_or_else(|_| "[]".into())],
    )?;
    Ok(())
}

// ------------------------------------------------------------------ ignorierte Befunde

pub fn ignored(conn: &Connection, project_id: i64) -> rusqlite::Result<Vec<(String, String)>> {
    let mut st = conn.prepare("SELECT finding, ignored_at FROM maintenance_ignored WHERE project_id = ?1 ORDER BY ignored_at DESC, finding")?;
    let v = st.query_map([project_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    Ok(v)
}

pub fn ignore(conn: &Connection, project_id: i64, finding_ids: &[String]) -> rusqlite::Result<()> {
    for id in finding_ids {
        conn.execute("INSERT OR IGNORE INTO maintenance_ignored (project_id, finding, ignored_at) VALUES (?1, ?2, ?3)", params![project_id, id, super::common::now()])?;
    }
    Ok(())
}

pub fn unignore(conn: &Connection, project_id: i64, finding_ids: &[String]) -> rusqlite::Result<()> {
    for id in finding_ids {
        conn.execute("DELETE FROM maintenance_ignored WHERE project_id = ?1 AND finding = ?2", params![project_id, id])?;
    }
    Ok(())
}

// ------------------------------------------------------------------ Protokoll

#[derive(Debug, Clone, PartialEq)]
pub struct LogEntry {
    pub id: i64,
    pub project_key: String,
    pub at: String,
    /// Kennung der Aktion (`close_wont_do`, `archive`, `compact` …).
    pub action: String,
    pub count: i64,
    /// Dateiname der Sicherung vor der Aktion.
    pub backup: Option<String>,
    pub detail: String,
}

pub fn write_log(conn: &Connection, project_id: Option<i64>, action: &str, count: i64, backup: Option<&str>, detail: &str) -> rusqlite::Result<i64> {
    let key: String = match project_id {
        Some(p) => conn.query_row("SELECT key FROM projects WHERE id = ?1", [p], |r| r.get(0)).optional()?.unwrap_or_default(),
        None => String::new(),
    };
    conn.execute(
        "INSERT INTO maintenance_log (project_id, project_key, at, action, count, backup, detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![project_id, key, super::common::now(), action, count, backup, detail],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Protokoll eines Projekts (mit den Einträgen für die ganze Datenbank), neueste zuerst.
pub fn read_log(conn: &Connection, project_id: i64) -> rusqlite::Result<Vec<LogEntry>> {
    let mut st = conn.prepare("SELECT id, project_key, at, action, count, backup, detail FROM maintenance_log WHERE project_id = ?1 OR project_id IS NULL ORDER BY id DESC")?;
    let v = st
        .query_map([project_id], |r| Ok(LogEntry { id: r.get(0)?, project_key: r.get(1)?, at: r.get(2)?, action: r.get(3)?, count: r.get(4)?, backup: r.get(5)?, detail: r.get(6)? }))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(v)
}

/// Protokoll als CSV (Export auf der Seite): Kopfzeile, dann je Eintrag eine Zeile.
pub fn log_csv(entries: &[LogEntry]) -> String {
    let esc = |s: &str| if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() };
    let mut out = String::from("at,project,action,count,backup,detail\r\n");
    for e in entries {
        out.push_str(&format!("{},{},{},{},{},{}\r\n", esc(&e.at), esc(&e.project_key), esc(&e.action), e.count, esc(e.backup.as_deref().unwrap_or("")), esc(&e.detail)));
    }
    out
}

/// Anzeigename einer Aktion im Protokoll.
pub fn action_label(action: &str, lang: Lang) -> String {
    match action {
        "close_wont_do" => lang.t("Als Won't Do geschlossen", "Closed as Won't Do").into(),
        "archive" => lang.t("Archiviert", "Archived").into(),
        "unarchive" => lang.t("Wiederhergestellt (Archiv)", "Restored from archive").into(),
        "set_due" => lang.t("Fälligkeit gesetzt", "Due date set").into(),
        "link_duplicate" => lang.t("Als Duplikat verknüpft", "Linked as duplicate").into(),
        "remove_config" => lang.t("Ungenutzte Konfiguration entfernt", "Unused configuration removed").into(),
        "merge_labels" => lang.t("Labels zusammengeführt", "Labels merged").into(),
        "detach_parent" => lang.t("Verwaiste Unteraufgaben gelöst", "Orphaned subtasks detached").into(),
        "clear_reference" => lang.t("Kaputte Verweise entfernt", "Broken references removed").into(),
        "rule_action" => lang.t("Aktion einer eigenen Regel", "Custom rule action").into(),
        "repair" => lang.t("Datenbank repariert", "Database repaired").into(),
        "compact" => lang.t("Datenbank verdichtet", "Database compacted").into(),
        "rescue" => lang.t("Daten in neue Datei gerettet", "Data rescued into a new file").into(),
        other => other.to_string(),
    }
}

// ------------------------------------------------------------------ Bericht

/// Wie geprüft wird.
#[derive(Debug, Clone)]
pub struct RunOpts {
    pub now: chrono::NaiveDateTime,
    pub lang: Lang,
    /// Mögliche Duplikate suchen (Textvergleich, teurer).
    pub duplicates: bool,
    /// Datenbank-Prüfungen (Integrität, Fremdschlüssel, Zähler). `quick`: `quick_check` statt `integrity_check`.
    pub database: Option<DbScope>,
    /// Eigene Regeln ausführen.
    pub rules: bool,
    /// Auch ignorierte Befunde liefern.
    pub include_ignored: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbScope {
    /// Schnelle Prüfung (`quick_check`), z. B. für MCP.
    Quick,
    /// Vollständige Prüfung (`integrity_check`).
    Full,
}

impl RunOpts {
    /// Volle Prüfung (Seite öffnen).
    pub fn full(now: chrono::NaiveDateTime, lang: Lang) -> RunOpts {
        RunOpts { now, lang, duplicates: true, database: Some(DbScope::Full), rules: true, include_ignored: false }
    }

    /// Günstige Zählung (Übersicht): ohne Duplikat-Suche und ohne Datenbank-Prüfung.
    pub fn cheap(now: chrono::NaiveDateTime, lang: Lang) -> RunOpts {
        RunOpts { now, lang, duplicates: false, database: None, rules: true, include_ignored: false }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub project_id: i64,
    pub project_key: String,
    pub generated_at: String,
    pub findings: Vec<Finding>,
    /// Ausgeblendete (ignorierte) Befunde.
    pub ignored: usize,
    /// Ausgelassene Prüfungen samt Grund (z. B. abgeschaltet), nur zur Anzeige.
    pub disabled: Vec<Check>,
}

impl Report {
    pub fn count(&self, c: Category) -> usize {
        self.findings.iter().filter(|f| f.category() == c).count()
    }

    pub fn count_severity(&self, s: Severity) -> usize {
        self.findings.iter().filter(|f| f.severity == s).count()
    }
}

/// Ein Fehler der Prüfung (Datenbank nicht lesbar o. ä.).
#[derive(Debug, Clone, PartialEq)]
pub enum MaintError {
    NotFound,
    /// System-Projekt (Todolist): keine Wartung.
    SystemProject,
    Db(String),
    /// Aktion nicht möglich (Grund in beiden Sprachen).
    Refused { de: String, en: String },
}

impl MaintError {
    pub fn message(&self, lang: Lang) -> String {
        match self {
            MaintError::NotFound => lang.t("Nicht gefunden.", "Not found.").into(),
            MaintError::SystemProject => lang.t("Das System-Projekt TODO hat keine Wartung.", "The system project TODO has no maintenance.").into(),
            MaintError::Db(e) => format!("{}: {e}", lang.t("Datenbankfehler", "Database error")),
            MaintError::Refused { de, en } => match lang {
                Lang::De => de.clone(),
                Lang::En => en.clone(),
            },
        }
    }

    pub fn refused(de: impl Into<String>, en: impl Into<String>) -> MaintError {
        MaintError::Refused { de: de.into(), en: en.into() }
    }
}

impl From<rusqlite::Error> for MaintError {
    fn from(e: rusqlite::Error) -> Self {
        MaintError::Db(e.to_string())
    }
}

impl From<super::PlanError> for MaintError {
    fn from(e: super::PlanError) -> Self {
        match e {
            super::PlanError::NotFound => MaintError::NotFound,
            other => MaintError::Refused { de: other.message(Lang::De), en: other.message(Lang::En) },
        }
    }
}

impl From<super::ticket_error::TicketError> for MaintError {
    fn from(e: super::ticket_error::TicketError) -> Self {
        MaintError::Refused { de: e.message(Lang::De), en: e.message(Lang::En) }
    }
}

pub type MRes<T> = Result<T, MaintError>;

/// Das Projekt für die Wartung: nicht das System-Projekt.
pub fn maint_project(conn: &Connection, project_id: i64) -> MRes<super::projects::Project> {
    let p = super::projects::get_project(conn, project_id).map_err(|_| MaintError::NotFound)?;
    if p.system {
        return Err(MaintError::SystemProject);
    }
    Ok(p)
}

/// Führt alle eingeschalteten Prüfungen für das Projekt aus. Nur lesend.
pub fn run_report(conn: &Connection, project_id: i64, o: &RunOpts) -> MRes<Report> {
    let project = maint_project(conn, project_id)?;
    let settings = load_settings(conn, project_id)?;
    let mut findings = checks_tickets::run(conn, &project, &settings, o)?;
    findings.extend(checks_config::run(conn, &project, &settings, o)?);
    if let Some(scope) = o.database {
        findings.extend(dbcare::findings(conn, scope, o.lang)?);
    }
    if o.rules {
        findings.extend(rules::run(conn, &project, o)?);
    }
    findings.retain(|f| !f.check.switchable() || settings.enabled(f.check));
    let ignored_ids: std::collections::HashSet<String> = ignored(conn, project_id)?.into_iter().map(|(id, _)| id).collect();
    let before = findings.len();
    if !o.include_ignored {
        findings.retain(|f| !ignored_ids.contains(&f.id));
    }
    let ignored_count = if o.include_ignored { findings.iter().filter(|f| ignored_ids.contains(&f.id)).count() } else { before - findings.len() };
    findings.sort_by(|a, b| (a.check.category(), a.severity, a.check, &a.id).cmp(&(b.check.category(), b.severity, b.check, &b.id)));
    Ok(Report {
        project_id,
        project_key: project.key,
        generated_at: o.now.format("%Y-%m-%dT%H:%M:%S").to_string(),
        findings,
        ignored: ignored_count,
        disabled: settings.disabled.clone(),
    })
}

/// Zahl der Probleme (Fehler und Warnungen; Hinweise zählen nicht) für den Knopf „Wartung (12)“ der Übersicht – günstig:
/// ohne Duplikat-Suche und ohne Datenbank-Prüfung.
pub fn quick_count(conn: &Connection, project_id: i64, now: chrono::NaiveDateTime) -> MRes<usize> {
    Ok(run_report(conn, project_id, &RunOpts::cheap(now, Lang::En))?.findings.iter().filter(|f| f.severity != Severity::Hint).count())
}

// ------------------------------------------------------------------ gemeinsame Hilfen der Prüfungen

/// Tickets des Projekts für die Prüfungen (ohne archivierte).
#[derive(Debug, Clone)]
pub(crate) struct TRow {
    pub id: i64,
    pub key: String,
    pub title: String,
    pub type_id: i64,
    pub status_id: Option<i64>,
    pub category: Option<super::statuses::StatusCategory>,
    pub parent_id: Option<i64>,
    pub updated_at: String,
}

impl TRow {
    pub fn tref(&self) -> TicketRef {
        TicketRef { id: self.id, key: self.key.clone(), title: self.title.clone() }
    }

    /// Offen = hat einen Status, der nicht „erledigt“ ist. Tickets ohne Workflow zählen nicht als offen.
    pub fn open(&self) -> bool {
        matches!(self.category, Some(c) if c != super::statuses::StatusCategory::Done)
    }

    pub fn done(&self) -> bool {
        self.category == Some(super::statuses::StatusCategory::Done)
    }
}

pub(crate) fn load_tickets(conn: &Connection, project: &super::projects::Project) -> rusqlite::Result<Vec<TRow>> {
    let mut st = conn.prepare(
        "SELECT t.id, t.number, t.title, t.type_id, t.status_id, s.category, t.parent_id, t.updated_at
         FROM tickets t LEFT JOIN statuses s ON s.id = t.status_id WHERE t.project_id = ?1 AND t.archived = 0 ORDER BY t.number",
    )?;
    let v = st
        .query_map([project.id], |r| {
            Ok(TRow {
                id: r.get(0)?,
                key: format!("{}-{}", project.key, r.get::<_, i64>(1)?),
                title: r.get(2)?,
                type_id: r.get(3)?,
                status_id: r.get(4)?,
                category: r.get::<_, Option<String>>(5)?.map(|c| super::statuses::StatusCategory::parse(&c)),
                parent_id: r.get(6)?,
                updated_at: r.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(v)
}

/// Ganze Tage zwischen einem gespeicherten Zeitpunkt (ISO) und jetzt.
pub(crate) fn days_since(stamp: &str, now: chrono::NaiveDateTime) -> Option<i64> {
    let t = chrono::NaiveDateTime::parse_from_str(stamp.get(..19)?, "%Y-%m-%dT%H:%M:%S").ok()?;
    Some(now.signed_duration_since(t).num_days())
}
