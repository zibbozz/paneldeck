//! Aufräumen (#307, 2.6.0): Was der Datenordner dieser Instanz (und was Paneldeck außerhalb davon) belegt, und das
//! Löschen der vom Nutzer gewählten Teile. Kein egui, keine App – die Oberfläche steht in `app::cleanup_ui`.
//!
//! * [`Item`]: die Einträge des Fensters, in Gruppen ([`Group`]). Jeder Eintrag hat eine Kennung (`id`), die auch das
//!   MCP-Werkzeug `cleanup_sizes` (nur lesend) und die Vormerkung für den Neustart benutzen.
//! * [`scan`]: Größen und Anzahlen ermitteln (im Hintergrund aufrufen). Optionale Einträge (Programm-Kopien von Läufen,
//!   Wartungs-Sicherungen, Hot-Exit-Puffer) erscheinen nur, wenn ihr Ordner bzw. ihre Dateien existieren – so kommen
//!   künftige Unterordner des Datenordners als weitere Einträge dazu, ohne das Fenster umzubauen.
//! * [`execute`]: löscht die Auswahl dateiweise. Gelöscht wird endgültig (kein Papierkorb). Geänderte JSON-Dateien und
//!   Registry-Einträge werden vorher gesichert. Vorher entsteht eine Datenbank-Sicherung (abschaltbar; erzwungen, wenn die
//!   Datenbank selbst gelöscht wird – schlägt sie fehl, bleibt die Datenbank, die übrigen Einträge laufen weiter).
//!   Jede Datei, die sich nicht löschen lässt (z. B. in Benutzung), wird übersprungen und im Ergebnis genannt.
//! * Was im laufenden Betrieb nicht geht (Datenbank, Sitzungen/Layout, alles zurücksetzen), wird als
//!   [`PENDING_FILE`] vorgemerkt und beim nächsten Start vor dem Laden ausgeführt ([`apply_pending`]).
//! * Nie berührt: die Daten von Claude Code selbst (`~/.claude/projects`, Transkripte).

pub mod integ;
pub mod zip;

#[cfg(test)]
mod tests;

use crate::state::Lang;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Vormerkung für den nächsten Start (im Datenordner).
pub const PENDING_FILE: &str = "cleanup-pending.json";
/// Ordner der Programm-Kopien von Läufen unter `%TEMP%` (#312).
pub const RUN_TEMP: &str = "paneldeck-run";
/// Ordner der Hot-Exit-Puffer des Editors im Datenordner (#54, `editor::HotExitStore`).
pub const HOT_EXIT: &str = "editor-hotexit";
/// Zustand des Editor-Reiters (offener Ordner, Tabs, Teilungen, zuletzt geöffnet; #54, intern) – gehört zu
/// „Sitzungen und Layout“.
pub const EDITOR_STATE: &str = "editor-state.json";
/// Beispiel-Theme, das immer bleibt (wird beim Start ohnehin neu angelegt).
pub const EXAMPLE_THEME: &str = "nord-beispiel.json";
/// Standard für „älter als X Tage“.
pub const DEFAULT_DAYS: u32 = 30;
/// Standard für „neueste N Sicherungen behalten“.
pub const DEFAULT_KEEP: usize = 3;

/// Gruppen des Fensters in Anzeigereihenfolge.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub enum Group {
    Space,
    History,
    Content,
    Settings,
    Integration,
    Everything,
}

impl Group {
    pub const ALL: [Group; 6] = [Group::Space, Group::History, Group::Content, Group::Settings, Group::Integration, Group::Everything];

    pub fn id(self) -> &'static str {
        match self {
            Group::Space => "space",
            Group::History => "history",
            Group::Content => "content",
            Group::Settings => "settings",
            Group::Integration => "integration",
            Group::Everything => "everything",
        }
    }

    pub fn label(self, l: Lang) -> &'static str {
        match self {
            Group::Space => l.t("Platz schaffen", "Save space"),
            Group::History => l.t("Verlauf und Protokolle", "History and logs"),
            Group::Content => l.t("Inhalte", "Content"),
            Group::Settings => l.t("Einstellungen", "Settings"),
            Group::Integration => l.t("Windows-Integration", "Windows integration"),
            Group::Everything => l.t("Alles", "Everything"),
        }
    }
}

/// Ein Eintrag des Fensters.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub enum Item {
    DbBackups,
    MaintenanceBackups,
    Clipboard,
    OldBackups,
    RunTemp,
    HotExit,
    ScrollbackOrphaned,
    ScrollbackAll,
    ChatsOrphaned,
    ChatsAll,
    CrashLog,
    McpLog,
    Prompts,
    Templates,
    Themes,
    SystemPrompt,
    Database,
    Sessions,
    SettingsAll,
    SettingsModels,
    SettingsClaude,
    SettingsKeys,
    Autostart,
    ContextMenu,
    ClaudeJson,
    Hooks,
    FactoryReset,
}

impl Item {
    pub const ALL: [Item; 27] = [
        Item::DbBackups,
        Item::MaintenanceBackups,
        Item::Clipboard,
        Item::OldBackups,
        Item::RunTemp,
        Item::HotExit,
        Item::ScrollbackOrphaned,
        Item::ScrollbackAll,
        Item::ChatsOrphaned,
        Item::ChatsAll,
        Item::CrashLog,
        Item::McpLog,
        Item::Prompts,
        Item::Templates,
        Item::Themes,
        Item::SystemPrompt,
        Item::Database,
        Item::Sessions,
        Item::SettingsAll,
        Item::SettingsModels,
        Item::SettingsClaude,
        Item::SettingsKeys,
        Item::Autostart,
        Item::ContextMenu,
        Item::ClaudeJson,
        Item::Hooks,
        Item::FactoryReset,
    ];

    /// Kennung (MCP, Vormerkung, Ergebnis) – nie ändern.
    pub fn id(self) -> &'static str {
        match self {
            Item::DbBackups => "db_backups",
            Item::MaintenanceBackups => "maintenance_backups",
            Item::Clipboard => "clipboard_images",
            Item::OldBackups => "old_backup_files",
            Item::RunTemp => "run_temp",
            Item::HotExit => "hot_exit",
            Item::ScrollbackOrphaned => "scrollback_orphaned",
            Item::ScrollbackAll => "scrollback_all",
            Item::ChatsOrphaned => "chats_orphaned",
            Item::ChatsAll => "chats_all",
            Item::CrashLog => "crash_log",
            Item::McpLog => "mcp_log",
            Item::Prompts => "prompts",
            Item::Templates => "templates",
            Item::Themes => "custom_themes",
            Item::SystemPrompt => "system_prompt",
            Item::Database => "database",
            Item::Sessions => "sessions_layout",
            Item::SettingsAll => "settings_all",
            Item::SettingsModels => "settings_models",
            Item::SettingsClaude => "settings_claude",
            Item::SettingsKeys => "settings_shortcuts",
            Item::Autostart => "autostart",
            Item::ContextMenu => "context_menu",
            Item::ClaudeJson => "claude_json",
            Item::Hooks => "hooks",
            Item::FactoryReset => "factory_reset",
        }
    }

    pub fn from_id(id: &str) -> Option<Item> {
        Item::ALL.into_iter().find(|i| i.id() == id)
    }

    pub fn group(self) -> Group {
        use Item::*;
        match self {
            DbBackups | MaintenanceBackups | Clipboard | OldBackups | RunTemp | HotExit => Group::Space,
            ScrollbackOrphaned | ScrollbackAll | ChatsOrphaned | ChatsAll | CrashLog | McpLog => Group::History,
            Prompts | Templates | Themes | SystemPrompt | Database | Sessions => Group::Content,
            SettingsAll | SettingsModels | SettingsClaude | SettingsKeys => Group::Settings,
            Autostart | ContextMenu | ClaudeJson | Hooks => Group::Integration,
            FactoryReset => Group::Everything,
        }
    }

    /// Vorausgewählt: nur Harmloses.
    pub fn preselected(self) -> bool {
        matches!(self, Item::Clipboard | Item::OldBackups | Item::CrashLog | Item::ScrollbackOrphaned | Item::ChatsOrphaned)
    }

    /// Unwiderruflich: verlangt das Eintippen von `DELETE`/`LÖSCHEN` (einmal je Lauf). `all_backups` = bei den
    /// Sicherungen ist „alle“ gewählt.
    pub fn needs_typing(self, all_backups: bool) -> bool {
        use Item::*;
        match self {
            DbBackups => all_backups,
            // Hot-Exit (#54): ungespeicherte Arbeit des Nutzers, nicht wiederherstellbar.
            HotExit | ScrollbackAll | ChatsAll | Prompts | Templates | Themes | SystemPrompt | Database | Sessions | SettingsAll | FactoryReset => true,
            _ => false,
        }
    }

    /// Geht nur ohne laufende Instanz: wird vorgemerkt und beim Neustart ausgeführt.
    pub fn needs_restart(self) -> bool {
        // Hot-Exit (#54): offene Tabs schreiben ihre Puffer beim Beenden neu – gelöscht wird beim nächsten Start, bevor der
        // Editor sie wiederherstellt.
        matches!(self, Item::Database | Item::Sessions | Item::FactoryReset | Item::HotExit)
    }

    /// Erscheint nur, wenn es etwas davon gibt (künftige Unterordner, #54/#308/#312).
    pub fn optional(self) -> bool {
        matches!(self, Item::MaintenanceBackups | Item::RunTemp | Item::HotExit)
    }

    /// Betrifft offene Sitzungen (Rückfrage nennt sie und laufende Programme).
    pub fn affects_sessions(self) -> bool {
        matches!(self, Item::ScrollbackAll | Item::ChatsAll | Item::Sessions | Item::FactoryReset)
    }

    /// Beendet offene Sitzungen (Neustart).
    pub fn ends_sessions(self) -> bool {
        matches!(self, Item::Sessions | Item::FactoryReset)
    }

    /// Außerhalb des Datenordners (Registry, `~/.claude.json`): nur im laufenden Betrieb, nie aus der Vormerkung.
    pub fn outside_data(self) -> bool {
        self.group() == Group::Integration
    }

    /// Einstellungs-Einträge (im laufenden Betrieb ändert die Oberfläche die geladenen Einstellungen).
    pub fn is_setting(self) -> bool {
        self.group() == Group::Settings
    }

    /// Der Dateialter-Filter („älter als X Tage“) gilt hierfür.
    pub fn age_filtered(self) -> bool {
        matches!(self, Item::ScrollbackOrphaned | Item::ScrollbackAll | Item::ChatsOrphaned | Item::ChatsAll)
    }

    pub fn label(self, l: Lang) -> &'static str {
        use Item::*;
        match self {
            DbBackups => l.t("Datenbank-Sicherungen", "Database backups"),
            MaintenanceBackups => l.t("Wartungs-Sicherungen", "Maintenance backups"),
            Clipboard => l.t("Bilder aus der Zwischenablage", "Clipboard images"),
            OldBackups => l.t("Alte Sicherungsdateien", "Old backup files"),
            RunTemp => l.t("Programm-Kopien von Läufen", "Program copies of runs"),
            HotExit => l.t("Editor: ungespeicherte Puffer", "Editor: unsaved buffers"),
            ScrollbackOrphaned => l.t("Scrollback ohne Sitzung", "Orphaned scrollback"),
            ScrollbackAll => l.t("Scrollback – alle", "Scrollback – all"),
            ChatsOrphaned => l.t("Chatverläufe ohne Sitzung", "Orphaned chat histories"),
            ChatsAll => l.t("Chatverläufe – alle", "Chat histories – all"),
            CrashLog => l.t("Absturzprotokoll (crash.log)", "Crash log (crash.log)"),
            McpLog => l.t("MCP-Protokoll (mcp-log.jsonl)", "MCP call log (mcp-log.jsonl)"),
            Prompts => l.t("Prompt-Bibliothek", "Prompt library"),
            Templates => l.t("Terminal- und Gruppenvorlagen", "Terminal and group templates"),
            Themes => l.t("Eigene Themes", "Custom themes"),
            SystemPrompt => l.t("Eigener Systemprompt (system-prompt.md)", "Custom system prompt (system-prompt.md)"),
            Database => l.t("Todo-/Ticket-Datenbank", "To-do/ticket database"),
            Sessions => l.t("Sitzungen und Layout", "Sessions and layout"),
            SettingsAll => l.t("Alle Einstellungen", "All settings"),
            SettingsModels => l.t("Modell-Server und API-Schlüssel", "Model servers and API keys"),
            SettingsClaude => l.t("Claude-Standards", "Claude defaults"),
            SettingsKeys => l.t("Tastenkürzel", "Keyboard shortcuts"),
            Autostart => l.t("Autostart", "Autostart"),
            ContextMenu => l.t("Explorer-Kontextmenü", "Explorer context menu"),
            ClaudeJson => l.t("Einträge in ~/.claude.json", "Entries in ~/.claude.json"),
            Hooks => l.t("Hook-Dateien für Claude Code", "Hook files for Claude Code"),
            FactoryReset => l.t("Paneldeck komplett zurücksetzen", "Reset Paneldeck completely"),
        }
    }

    pub fn hint(self, l: Lang) -> &'static str {
        use Item::*;
        match self {
            DbBackups => l.t("Alle außer den neuesten N im aktiven Sicherungsordner.", "All except the newest N in the active backup folder."),
            MaintenanceBackups => l.t("Wartungs-Sicherungen (maintenance-<Zeit>.db) im Sicherungsordner.", "Maintenance backups (maintenance-<time>.db) in the backup folder."),
            Clipboard => l.t("Eingefügte Bilder (clipboard\\).", "Pasted images (clipboard\\)."),
            OldBackups => l.t("*.bak-* und *.defekt-* im Datenordner.", "*.bak-* and *.defekt-* in the data folder."),
            RunTemp => l.t("Kopien der Programmdatei unter %TEMP%\\paneldeck-run.", "Copies of the program under %TEMP%\\paneldeck-run."),
            HotExit => l.t("Ungespeicherte Änderungen von Editor-Tabs (editor-hotexit\\) – sie gehen verloren. Neustart.", "Unsaved changes of editor tabs (editor-hotexit\\) – they are lost. Restart."),
            ScrollbackOrphaned => l.t("Gespeicherter Terminaltext, zu dem es keine Sitzung mehr gibt.", "Saved terminal text that no session belongs to any more."),
            ScrollbackAll => l.t("Auch offene Terminals: sie behalten nur den Bildschirm.", "Open terminals too: they keep only the screen."),
            ChatsOrphaned => l.t("Verläufe, zu denen es keinen Chat mehr gibt.", "Histories that no chat belongs to any more."),
            ChatsAll => l.t("Auch offene Chats: sie werden leer.", "Open chats too: they become empty."),
            CrashLog => l.t("Protokoll früherer Abstürze.", "Log of earlier crashes."),
            McpLog => l.t("Dokumentiert, was Agenten über MCP getan haben.", "Documents what agents did through MCP."),
            Prompts => l.t("prompts.json – zurück zu den Standard-Prompts.", "prompts.json – back to the default prompts."),
            Templates => l.t("templates.json", "templates.json"),
            Themes => l.t("Selbst angelegte Themes (themes\\*.json); eingebaute und das Beispiel bleiben.", "Themes you created (themes\\*.json); built-in ones and the example stay."),
            SystemPrompt => l.t("Zurück zur eingebauten Vorlage.", "Back to the built-in template."),
            Database => l.t("paneldeck.db – wird vorher immer gesichert. Neustart.", "paneldeck.db – always backed up first. Restart."),
            Sessions => l.t("state.json, Scrollback, Chats und die Tabs des Editors (editor-state.json); offene Sitzungen werden beendet. Neustart.", "state.json, scrollback, chats and the editor's tabs (editor-state.json); open sessions are ended. Restart."),
            SettingsAll => l.t("Wie --reset-settings (mit .bak-Kopie).", "Like --reset-settings (with a .bak copy)."),
            SettingsModels => l.t("Eingerichtete LLM-Server samt gespeicherter Schlüssel.", "Configured LLM servers including stored keys."),
            SettingsClaude => l.t("Standard-Startoptionen neuer Claude-Sitzungen.", "Default start options of new Claude sessions."),
            SettingsKeys => l.t("Eigene Tastenkürzel – zurück zu den Standards.", "Custom shortcuts – back to the defaults."),
            Autostart => l.t("Run-Werte (auch von Profilen), die auf diese paneldeck.exe zeigen.", "Run values (profiles too) that point to this paneldeck.exe."),
            ContextMenu => l.t("„In Paneldeck öffnen“ und „In Paneldeck-Editor öffnen“, wenn sie auf diese paneldeck.exe zeigen.", "“Open in Paneldeck” and “Open in Paneldeck editor”, if they point to this paneldeck.exe."),
            ClaudeJson => l.t("paneldeck*-MCP-Einträge, die auf diese .exe zeigen (Datei wird vorher gesichert).", "paneldeck* MCP entries pointing to this .exe (the file is backed up first)."),
            Hooks => l.t("claude-hooks*.json im Datenordner (auch für WSL).", "claude-hooks*.json in the data folder (also for WSL)."),
            FactoryReset => l.t("Alles oben, samt Integration. Vorher ZIP-Sicherung, danach Neustart wie beim ersten Start.", "Everything above, including integration. ZIP backup first, then a restart like the first start."),
        }
    }
}

/// Eine gefundene Datei (bzw. ein Ordner bei Profilen).
#[derive(Clone, Debug, PartialEq)]
pub struct FileEntry {
    pub path: PathBuf,
    pub bytes: u64,
    pub modified: Option<SystemTime>,
}

/// Ergebnis des Scans für einen Eintrag.
#[derive(Clone, Debug, Default)]
pub struct Found {
    /// Dateien (bei Sicherungen neueste zuerst).
    pub files: Vec<FileEntry>,
    /// Einträge außerhalb von Dateien (Registry-Werte, JSON-Einträge, Einstellungen) – für die Anzeige.
    pub entries: Vec<String>,
    /// Ordner, die selbst ein Link (Symlink/Junction) sind: nicht betreten und nicht berührt (im Ergebnis genannt).
    pub linked: Vec<PathBuf>,
}

impl Found {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.entries.is_empty() && self.linked.is_empty()
    }
}

/// Ein anderes Profil (`profiles\<name>` mit `settings.json`).
#[derive(Clone, Debug)]
pub struct ProfileInfo {
    pub name: String,
    pub path: PathBuf,
    pub bytes: u64,
    pub files: u64,
    /// Seine Instanz läuft (hält `instance.lock`): dann nicht löschbar.
    pub running: bool,
}

/// Alles, was der Scan gefunden hat.
#[derive(Clone, Debug, Default)]
pub struct Scan {
    pub items: Vec<(Item, Found)>,
    pub profiles: Vec<ProfileInfo>,
}

impl Scan {
    pub fn found(&self, item: Item) -> Option<&Found> {
        self.items.iter().find(|(i, _)| *i == item).map(|(_, f)| f)
    }

    /// Sichtbare Einträge: alle außer optionalen ohne Inhalt.
    pub fn visible(&self, item: Item) -> bool {
        self.found(item).is_some_and(|f| !item.optional() || !f.is_empty())
    }
}

/// Wo gesucht wird (im Programm aus dem laufenden Hub; in Tests frei).
#[derive(Clone, Default)]
pub struct Ctx {
    pub data_dir: PathBuf,
    /// Aktiver Sicherungsordner (auch `--backup-dir`).
    pub backup_dir: PathBuf,
    /// Ordner für die Sicherheitskopien der Integration (`.reg`, `claude.json-*.bak`), sonst `backup_dir`. Beim
    /// Zurücksetzen der Ordner der ZIP-Sicherung – `backup_dir` im Datenordner wird beim Neustart gelöscht.
    pub safety_dir: Option<PathBuf>,
    /// Datenordner ohne Profil (dort liegen `profiles\*`).
    pub base_dir: PathBuf,
    /// Sitzungen, die gerade offen sind (Dateien anderer Ids gelten als verwaist).
    pub open_ids: Vec<u64>,
    /// Diese paneldeck.exe (Integration zeigt nur dann auf „uns“).
    pub exe: PathBuf,
    /// `%TEMP%`.
    pub temp_dir: PathBuf,
    /// Geltende Einstellungen (im laufenden Betrieb die geladenen; sonst aus `settings.json`).
    pub settings: Option<crate::state::Settings>,
    /// Integration des Systems lesen (Registry, `~/.claude.json`). In Tests aus – dort kommt sie aus `integ_override`.
    pub system_integration: bool,
    /// Für Tests: Integration ohne System.
    pub integ_override: Option<integ::State>,
}

impl Ctx {
    /// Wohin die Sicherheitskopien der Integration kommen.
    pub fn safety_dir(&self) -> &Path {
        self.safety_dir.as_deref().unwrap_or(&self.backup_dir)
    }
}

/// Optionen eines Laufs.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Opts {
    /// Neueste N Datenbank-Sicherungen behalten (0 = alle löschen, nur mit Eintippen).
    pub keep_backups: usize,
    /// Nur Verlauf (Scrollback/Chats) älter als so viele Tage.
    pub older_than_days: Option<u32>,
    /// Vorher die Datenbank sichern (erzwungen, wenn sie selbst gelöscht wird).
    pub backup_db: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Opts { keep_backups: DEFAULT_KEEP, older_than_days: None, backup_db: true }
    }
}

/// Ergebnis je Eintrag (auch über den Neustart hinweg, deshalb serialisierbar).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ItemReport {
    /// Kennung des Eintrags, `profile:<name>`, `db_backup` (die Sicherung vorher) oder `zip_backup`.
    pub id: String,
    pub bytes: u64,
    pub files: u64,
    /// Entfernte Einträge außerhalb von Dateien (Registry-Werte, JSON-Einträge).
    pub entries: u64,
    /// Fehler (eine Zeile je Datei/Schritt, z. B. „Datei in Benutzung“).
    pub errors: Vec<String>,
    /// Hinweise (Deutsch, Englisch), z. B. Ort einer Sicherung.
    pub notes: Vec<(String, String)>,
    /// Wirkt erst nach einem Neustart vollständig.
    pub restart: bool,
}

impl ItemReport {
    pub fn new(id: &str) -> Self {
        ItemReport { id: id.to_string(), ..Default::default() }
    }

    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }

    fn note(&mut self, de: impl Into<String>, en: impl Into<String>) {
        self.notes.push((de.into(), en.into()));
    }

    /// Anzeigename (Eintrag, Profil oder Sicherung).
    pub fn label(&self, l: Lang) -> String {
        if let Some(i) = Item::from_id(&self.id) {
            return i.label(l).to_string();
        }
        if let Some(p) = self.id.strip_prefix("profile:") {
            return format!("{} {}", l.t("Profil", "Profile"), l.q(p));
        }
        match self.id.as_str() {
            "db_backup" => l.t("Datenbank-Sicherung vorher", "Database backup first").to_string(),
            "zip_backup" => l.t("ZIP-Sicherung des Datenordners", "ZIP backup of the data folder").to_string(),
            other => other.to_string(),
        }
    }
}

/// Auswahl eines Laufs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    pub items: BTreeSet<Item>,
    /// Andere Profile, die ganz gelöscht werden.
    pub profiles: BTreeSet<String>,
}

impl Selection {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty() && self.profiles.is_empty()
    }

    /// Verlangt die Auswahl das Eintippen?
    pub fn needs_typing(&self, opts: &Opts) -> bool {
        !self.profiles.is_empty() || self.items.iter().any(|i| i.needs_typing(opts.keep_backups == 0))
    }

    pub fn needs_restart(&self) -> bool {
        self.items.iter().any(|i| i.needs_restart())
    }

    /// „Alles zurücksetzen“ wählt alles (ohne andere Profile).
    pub fn expanded(&self) -> BTreeSet<Item> {
        if self.items.contains(&Item::FactoryReset) {
            Item::ALL.into_iter().collect()
        } else {
            self.items.clone()
        }
    }
}

/// Bestätigungswort je Sprache.
pub fn confirm_word(l: Lang) -> &'static str {
    l.t("LÖSCHEN", "DELETE")
}

/// Stimmt die Eingabe? Beide Wörter gelten (die Sprache kann zwischen Anzeige und Eingabe wechseln), Groß-/Kleinschreibung
/// und Leerraum außen egal.
pub fn confirm_ok(input: &str) -> bool {
    let s = input.trim().to_uppercase();
    s == "DELETE" || s == "LÖSCHEN" || s == "LOESCHEN"
}

/// Größe lesbar (B, KB, MB, GB).
pub fn fmt_bytes(b: u64) -> String {
    const K: f64 = 1024.0;
    let f = b as f64;
    if b < 1024 {
        format!("{b} B")
    } else if f < K * K {
        format!("{:.0} KB", f / K)
    } else if f < K * K * K {
        format!("{:.1} MB", f / (K * K))
    } else {
        format!("{:.2} GB", f / (K * K * K))
    }
}

/// „312 MB, 48 Dateien“.
pub fn fmt_amount(bytes: u64, files: u64, entries: u64, l: Lang) -> String {
    let mut parts = Vec::new();
    if files > 0 || entries == 0 {
        parts.push(fmt_bytes(bytes));
        parts.push(format!("{files} {}", if files == 1 { l.t("Datei", "file") } else { l.t("Dateien", "files") }));
    }
    if entries > 0 {
        parts.push(format!("{entries} {}", if entries == 1 { l.t("Eintrag", "entry") } else { l.t("Einträge", "entries") }));
    }
    parts.join(", ")
}

// ---------------------------------------------------------------------------------------------------------------------
// Scan

fn entry_of(path: PathBuf, m: &std::fs::Metadata) -> FileEntry {
    FileEntry { path, bytes: m.len(), modified: m.modified().ok() }
}

/// Ist das ein Link bzw. Analysepunkt? Dann nicht hineingehen (und nie dem Ziel folgen).
fn is_link(m: &std::fs::Metadata) -> bool {
    m.file_type().is_symlink() || crate::platform::fs::is_reparse_point(m)
}

/// Ist der Ordner `dir` selbst ein Link (Symlink, Junction, anderer Analysepunkt)? Fehlt er, `false`.
pub fn root_is_link(dir: &Path) -> bool {
    std::fs::symlink_metadata(dir).is_ok_and(|m| is_link(&m))
}

/// Alle Dateien unter `dir` (rekursiv, ohne Links zu folgen). Ist `dir` selbst ein Link, nichts (nie dem Ziel folgen).
pub fn walk(dir: &Path) -> Vec<FileEntry> {
    if root_is_link(dir) {
        return Vec::new();
    }
    walk_followed(dir)
}

/// Ein Unterordner als Fund: seine Dateien – oder, ist er selbst ein Link, nichts und der Ordner in `linked`.
fn sub_tree(dir: &Path) -> Found {
    if root_is_link(dir) {
        return Found { linked: vec![dir.to_path_buf()], ..Default::default() };
    }
    Found { files: walk_followed(dir), ..Default::default() }
}

/// Wie [`walk`], aber der Wurzel `dir` selbst wird gefolgt (Datenordner und Sicherungsordner dürfen verlinkt sein, z. B.
/// auf ein anderes Laufwerk verschoben). Darunter wird keinem Link gefolgt.
fn walk_followed(dir: &Path) -> Vec<FileEntry> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(m) = std::fs::symlink_metadata(e.path()) else { continue };
            if m.is_dir() && !is_link(&m) {
                stack.push(e.path());
            } else {
                out.push(entry_of(e.path(), &m));
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Dateien direkt in `dir`, deren Name `keep` erfüllt. Ist `dir` selbst ein Link, nichts.
fn files_in(dir: &Path, keep: impl Fn(&str) -> bool) -> Vec<FileEntry> {
    if root_is_link(dir) {
        return Vec::new();
    }
    files_in_followed(dir, keep)
}

/// Wie [`files_in`], aber der Wurzel `dir` selbst wird gefolgt (Datenordner, Sicherungsordner).
fn files_in_followed(dir: &Path, keep: impl Fn(&str) -> bool) -> Vec<FileEntry> {
    let mut out: Vec<FileEntry> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let m = std::fs::symlink_metadata(e.path()).ok()?;
            (m.is_file() && keep(&e.file_name().to_string_lossy())).then(|| entry_of(e.path(), &m))
        })
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

fn one_file(p: &Path) -> Vec<FileEntry> {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.is_file() => vec![entry_of(p.to_path_buf(), &m)],
        _ => Vec::new(),
    }
}

/// Sitzungs-Id einer Verlaufsdatei (`<id>.txt`, `<id>.json`, `<id>.defekt-…`, `<id>.orphan-…`); `None` für fremde Dateien.
pub fn session_id_of(name: &str) -> Option<u64> {
    let digits: String = name.chars().take_while(|c| c.is_ascii_digit()).collect();
    let rest = &name[digits.len()..];
    let id = digits.parse::<u64>().ok()?;
    (id > 0 && rest.starts_with('.')).then_some(id)
}

/// Verlaufsdateien in `sub` (`scrollback` oder `chats`): (verwaist, alle, Ordner als Link übersprungen).
fn history_files(data_dir: &Path, sub: &str, open: &[u64]) -> (Vec<FileEntry>, Vec<FileEntry>, Vec<PathBuf>) {
    let dir = data_dir.join(sub);
    let linked: Vec<PathBuf> = if root_is_link(&dir) { vec![dir.clone()] } else { Vec::new() };
    let all = files_in(&dir, |n| session_id_of(n).is_some());
    let orphaned = all
        .iter()
        .filter(|f| {
            let name = f.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            // Beiseitegelegte fremde Verläufe (`.orphan-`) gehören nie zu einer offenen Sitzung.
            session_id_of(&name).is_some_and(|id| !open.contains(&id)) || name.contains(".orphan-")
        })
        .cloned()
        .collect();
    (orphaned, all, linked)
}

/// Wartungs-Sicherung von Paneldeck (#308)? Genau `maintenance-JJJJMMTT-HHMMSS[-<n>].db` mit gültigem Datum – so streng
/// wie die Datenbank-Sicherungen (`backup::list`); andere Dateien mit „maintenance“ im Namen gehören nicht dazu.
pub fn is_maintenance_backup_name(n: &str) -> bool {
    let Some(stem) = n.strip_suffix(".db") else { return false };
    let parts: Vec<&str> = stem.split('-').collect();
    let digits = |s: &str, len: usize| s.len() == len && s.chars().all(|c| c.is_ascii_digit());
    let counter_ok = match parts.len() {
        3 => true,
        4 => !parts[3].is_empty() && parts[3].chars().all(|c| c.is_ascii_digit()),
        _ => false,
    };
    counter_ok && parts[0] == "maintenance" && digits(parts[1], 8) && digits(parts[2], 6) && chrono::NaiveDateTime::parse_from_str(&format!("{}-{}", parts[1], parts[2]), "%Y%m%d-%H%M%S").is_ok()
}

/// Alte Sicherungsdateien im Datenordner.
pub fn is_old_backup_name(n: &str) -> bool {
    n.contains(".bak-") || n.contains(".defekt-") || n == "state.defekt.json" || n == crate::mcp::sysprompt::BACKUP_NAME || n.ends_with(".json.bak")
}

/// Gehört eine Datei zur Datenbank (`paneldeck.db`, `-wal`, `-shm`)?
fn db_files(data_dir: &Path) -> Vec<FileEntry> {
    ["", "-wal", "-shm"].iter().flat_map(|s| one_file(&data_dir.join(format!("{}{s}", crate::db::FILE)))).collect()
}

/// Wird diese Datei beim Zurücksetzen (und in der ZIP-Sicherung) ausgelassen? Sperre, IPC-Datei (Token), Vormerkung.
fn reset_keeps(rel: &Path) -> bool {
    let first = rel.components().next().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).unwrap_or_default();
    matches!(first.as_str(), "instance.lock" | "ipc.json" | "profiles") || first == PENDING_FILE
}

/// Alle Dateien dieser Instanz im Datenordner (ohne andere Profile, Sperre und IPC-Datei).
pub fn data_files(data_dir: &Path) -> Vec<FileEntry> {
    walk_followed(data_dir).into_iter().filter(|f| f.path.strip_prefix(data_dir).map(|r| !reset_keeps(r)).unwrap_or(false)).collect()
}

fn settings_of(ctx: &Ctx) -> crate::state::Settings {
    ctx.settings.clone().unwrap_or_else(|| crate::state::load_json(&ctx.data_dir.join("settings.json")).unwrap_or_default())
}

/// Hält eine andere Instanz die Sperre dieses Datenordners?
pub fn instance_running(dir: &Path) -> bool {
    let Ok(f) = std::fs::OpenOptions::new().write(true).open(dir.join("instance.lock")) else { return false };
    matches!(f.try_lock(), Err(std::fs::TryLockError::WouldBlock))
}

/// Andere Profile (`<base>\profiles\*` mit `settings.json`), ohne das eigene.
pub fn profiles(ctx: &Ctx) -> Vec<ProfileInfo> {
    let root = ctx.base_dir.join("profiles");
    let mut v: Vec<ProfileInfo> = std::fs::read_dir(&root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let m = std::fs::symlink_metadata(e.path()).ok()?;
            let p = e.path();
            if !m.is_dir() || is_link(&m) || !p.join("settings.json").is_file() || crate::launch::norm_path(&p.to_string_lossy()) == crate::launch::norm_path(&ctx.data_dir.to_string_lossy()) {
                return None;
            }
            let files = walk(&p);
            Some(ProfileInfo { name: e.file_name().to_string_lossy().to_string(), bytes: files.iter().map(|f| f.bytes).sum(), files: files.len() as u64, running: instance_running(&p), path: p })
        })
        .collect();
    v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    v
}

/// Größen und Anzahlen aller Einträge ermitteln (blockierend; im Hintergrund aufrufen).
pub fn scan(ctx: &Ctx) -> Scan {
    let d = &ctx.data_dir;
    let settings = settings_of(ctx);
    let integ_state = match &ctx.integ_override {
        Some(s) => s.clone(),
        None if ctx.system_integration => integ::State::read(&ctx.exe, &ctx.data_dir),
        None => integ::State::default(),
    };
    let backups = crate::backup::list(&ctx.backup_dir);
    let backup_paths: Vec<&Path> = backups.iter().map(|b| b.path.as_path()).collect();
    let (sb_orph, sb_all, sb_link) = history_files(d, "scrollback", &ctx.open_ids);
    let (ch_orph, ch_all, ch_link) = history_files(d, "chats", &ctx.open_ids);
    let themes_dir = d.join("themes");
    let mut items = Vec::new();
    for item in Item::ALL {
        let f = match item {
            Item::DbBackups => Found {
                files: backups.iter().filter(|b| b.kind != crate::backup::Kind::PreMaintenance).map(|b| FileEntry { path: b.path.clone(), bytes: b.size, modified: std::fs::metadata(&b.path).and_then(|m| m.modified()).ok() }).collect(),
                ..Default::default()
            },
            Item::MaintenanceBackups => Found {
                files: files_in_followed(&ctx.backup_dir, is_maintenance_backup_name).into_iter().filter(|f| backup_paths.contains(&f.path.as_path())).collect(),
                ..Default::default()
            },
            Item::Clipboard => sub_tree(&d.join("clipboard")),
            Item::OldBackups => Found { files: files_in_followed(d, is_old_backup_name), ..Default::default() },
            Item::RunTemp => {
                if ctx.temp_dir.as_os_str().is_empty() {
                    Found::default()
                } else {
                    sub_tree(&ctx.temp_dir.join(RUN_TEMP))
                }
            }
            Item::HotExit => sub_tree(&d.join(HOT_EXIT)),
            Item::ScrollbackOrphaned => Found { files: sb_orph.clone(), linked: sb_link.clone(), ..Default::default() },
            Item::ScrollbackAll => Found { files: sb_all.clone(), linked: sb_link.clone(), ..Default::default() },
            Item::ChatsOrphaned => Found { files: ch_orph.clone(), linked: ch_link.clone(), ..Default::default() },
            Item::ChatsAll => Found { files: ch_all.clone(), linked: ch_link.clone(), ..Default::default() },
            Item::CrashLog => Found { files: files_in_followed(d, |n| n == "crash.log" || n.starts_with("crash.log.")), ..Default::default() },
            Item::McpLog => Found { files: files_in_followed(d, |n| n == "mcp-log.jsonl" || n == "mcp-log.old.jsonl"), ..Default::default() },
            Item::Prompts => Found { files: one_file(&d.join("prompts.json")), ..Default::default() },
            Item::Templates => Found { files: one_file(&d.join("templates.json")), ..Default::default() },
            Item::Themes => Found {
                files: files_in(&themes_dir, |n| n.to_lowercase().ends_with(".json") && n != EXAMPLE_THEME),
                linked: if root_is_link(&themes_dir) { vec![themes_dir.clone()] } else { Vec::new() },
                ..Default::default()
            },
            Item::SystemPrompt => Found { files: one_file(&d.join(crate::mcp::sysprompt::FILE_NAME)), ..Default::default() },
            Item::Database => Found { files: db_files(d), ..Default::default() },
            Item::Sessions => {
                let mut files = one_file(&d.join("state.json"));
                // Tabs, Teilungen und „zuletzt geöffnet“ des Editor-Reiters (#54).
                files.extend(one_file(&d.join(EDITOR_STATE)));
                files.extend(sb_all.iter().cloned());
                files.extend(ch_all.iter().cloned());
                Found { files, linked: sb_link.iter().chain(ch_link.iter()).cloned().collect(), ..Default::default() }
            }
            Item::SettingsAll => Found { files: one_file(&d.join("settings.json")), ..Default::default() },
            Item::SettingsModels => Found { entries: settings.llm_servers.list.iter().map(|s| s.label().to_string()).chain(settings.llm_servers.unreadable.iter().map(|_| "?".to_string())).collect(), ..Default::default() },
            Item::SettingsClaude => Found {
                entries: if settings.claude_defaults.is_empty() && settings.last_claude_opts.is_none() { Vec::new() } else { vec!["claude_defaults".into()] },
                ..Default::default()
            },
            Item::SettingsKeys => Found { entries: settings.keys.keys().cloned().collect(), ..Default::default() },
            Item::Autostart => Found { entries: integ_state.run_values.iter().map(|(n, _)| n.clone()).collect(), ..Default::default() },
            Item::ContextMenu => Found { entries: integ_state.context_entries(), ..Default::default() },
            Item::ClaudeJson => Found { entries: integ_state.claude_json_servers.clone(), ..Default::default() },
            Item::Hooks => Found { files: files_in_followed(d, |n| n.starts_with("claude-hooks") && n.ends_with(".json")), ..Default::default() },
            Item::FactoryReset => {
                let mut files = data_files(d);
                // Ein eigener Sicherungsordner außerhalb des Datenordners: dessen Datenbank-Sicherungen gehören dazu.
                if !ctx.backup_dir.starts_with(d) {
                    files.extend(backups.iter().map(|b| FileEntry { path: b.path.clone(), bytes: b.size, modified: None }));
                }
                let entries = integ_state.run_values.len() + integ_state.context_entries().len() + integ_state.claude_json_servers.len();
                Found { files, entries: (0..entries).map(|i| i.to_string()).collect(), ..Default::default() }
            }
        };
        items.push((item, f));
    }
    Scan { items, profiles: profiles(ctx) }
}

/// Dateien, die mit diesen Optionen gelöscht würden (Sicherungen: alle außer den neuesten N; Verlauf: Altersfilter).
pub fn selected_files<'a>(item: Item, found: &'a Found, opts: &Opts, now: SystemTime) -> Vec<&'a FileEntry> {
    match item {
        Item::DbBackups => found.files.iter().skip(opts.keep_backups).collect(),
        i if i.age_filtered() => match opts.older_than_days {
            Some(days) => {
                let limit = now.checked_sub(Duration::from_secs(days as u64 * 86_400)).unwrap_or(SystemTime::UNIX_EPOCH);
                found.files.iter().filter(|f| f.modified.is_some_and(|m| m < limit)).collect()
            }
            None => found.files.iter().collect(),
        },
        _ => found.files.iter().collect(),
    }
}

/// Größe, Dateien und Einträge, die ein Eintrag mit diesen Optionen freigibt.
pub fn amount(item: Item, found: &Found, opts: &Opts, now: SystemTime) -> (u64, u64, u64) {
    let files = selected_files(item, found, opts, now);
    (files.iter().map(|f| f.bytes).sum(), files.len() as u64, found.entries.len() as u64)
}

/// Summe über die Auswahl (Dateien doppelt gewählter Einträge zählen einmal).
pub fn total(scan: &Scan, sel: &Selection, opts: &Opts, now: SystemTime) -> (u64, u64) {
    let mut seen: BTreeSet<&Path> = BTreeSet::new();
    let (mut bytes, mut files) = (0, 0);
    for item in sel.expanded() {
        let Some(f) = scan.found(item) else { continue };
        for e in selected_files(item, f, opts, now) {
            if seen.insert(&e.path) {
                bytes += e.bytes;
                files += 1;
            }
        }
    }
    for p in scan.profiles.iter().filter(|p| sel.profiles.contains(&p.name)) {
        bytes += p.bytes;
        files += p.files;
    }
    (bytes, files)
}

// ---------------------------------------------------------------------------------------------------------------------
// Löschen

/// Dateien löschen (endgültig). Fehlende gelten als erledigt; jede andere fehlgeschlagene Datei steht in `errors`.
pub fn delete_files<'a>(files: impl IntoIterator<Item = &'a FileEntry>, r: &mut ItemReport) {
    for f in files {
        match std::fs::remove_file(&f.path) {
            Ok(()) => {
                r.bytes += f.bytes;
                r.files += 1;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => r.errors.push(format!("{}: {e}", f.path.display())),
        }
    }
}

/// Leere Unterordner unter `dir` entfernen (nach dem Löschen von Dateien; `dir` selbst bleibt). Ist `dir` ein Link,
/// nichts (nie im Ziel aufräumen).
fn prune_empty_dirs(dir: &Path) {
    if root_is_link(dir) {
        return;
    }
    prune_empty_dirs_followed(dir);
}

/// Wie [`prune_empty_dirs`], aber der Wurzel selbst wird gefolgt (Datenordner).
fn prune_empty_dirs_followed(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(m) = std::fs::symlink_metadata(e.path()) else { continue };
        if m.is_dir() && !is_link(&m) {
            prune_empty_dirs(&e.path());
            let _ = std::fs::remove_dir(e.path());
        }
    }
}

fn stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

/// Freier Dateiname `<stem>-<zeit><ext>` in `dir`.
fn free_name(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let s = stamp();
    let mut p = dir.join(format!("{stem}-{s}{ext}"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{stem}-{s}-{n}{ext}"));
        n += 1;
    }
    p
}

/// Datenbank-Sicherung vor dem Lauf. `None`, wenn es keine Datenbank gibt.
fn backup_database(ctx: &Ctx) -> Option<(ItemReport, Option<PathBuf>)> {
    let db = ctx.data_dir.join(crate::db::FILE);
    if !db.is_file() {
        return None;
    }
    let mut r = ItemReport::new("db_backup");
    match crate::backup::create(&db, &ctx.backup_dir, crate::backup::Kind::PreCleanup) {
        Ok(b) => {
            r.bytes = b.size;
            r.files = 1;
            let p = b.path.display().to_string();
            r.note(format!("Gesichert: {p}"), format!("Saved: {p}"));
            Some((r, Some(b.path)))
        }
        Err(e) => {
            r.errors.push(e);
            Some((r, None))
        }
    }
}

/// Einstellungen eines Einstellungs-Eintrags zurücksetzen (im laufenden Betrieb auf den geladenen Einstellungen).
/// `SettingsAll` setzt alles auf die Standards; die Sprache bleibt, damit die Oberfläche lesbar bleibt, wenn nicht
/// ausdrücklich alles zurückgesetzt werden soll (`keep_language`).
pub fn reset_setting(item: Item, s: &mut crate::state::Settings, keep_language: bool) {
    match item {
        Item::SettingsAll => {
            let lang = s.language;
            *s = crate::state::Settings::default();
            if keep_language {
                s.language = lang;
            }
        }
        Item::SettingsModels => {
            s.llm_servers = Default::default();
            s.last_chat_model = None;
        }
        Item::SettingsClaude => {
            s.claude_defaults = Default::default();
            s.last_claude_opts = None;
            s.claude_bypass_dropped = false;
        }
        Item::SettingsKeys => s.keys.clear(),
        _ => {}
    }
}

/// Kopie `settings.json.bak-<zeit>` (wie `--reset-settings`, aber als Kopie).
pub fn backup_settings(data_dir: &Path) -> Result<Option<PathBuf>, String> {
    let src = data_dir.join("settings.json");
    if !src.is_file() {
        return Ok(None);
    }
    let mut target = data_dir.join(format!("settings.json.bak-{}", stamp()));
    let mut n = 2;
    while target.exists() {
        target = data_dir.join(format!("settings.json.bak-{}-{n}", stamp()));
        n += 1;
    }
    std::fs::copy(&src, &target).map_err(|e| format!("{}: {e}", target.display()))?;
    Ok(Some(target))
}

/// Ein anderes Profil löschen (nur, wenn seine Instanz nicht läuft).
pub fn delete_profile(p: &ProfileInfo, r: &mut ItemReport) {
    if instance_running(&p.path) {
        r.errors.push(crate::state::t("Die Instanz dieses Profils läuft – nicht gelöscht.", "This profile's instance is running – not deleted.").to_string());
        return;
    }
    let files = walk(&p.path);
    delete_files(&files, r);
    prune_empty_dirs(&p.path);
    if r.ok() {
        if let Err(e) = std::fs::remove_dir(&p.path) {
            r.errors.push(format!("{}: {e}", p.path.display()));
        }
    }
}

/// Wo dieser Lauf läuft.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// Im laufenden Hub: Einträge mit Neustart, Einstellungen (die ändert die Oberfläche) und die Datenbank fehlen hier.
    Live,
    /// Beim Start vor dem Laden (Vormerkung): keine Integration, keine Profile.
    Start,
}

/// Auswahl ausführen. `scan` muss frisch sein (Dateilisten). Gibt je Eintrag ein Ergebnis zurück; was in dieser Phase
/// nicht geht, wird übersprungen (die Oberfläche bzw. die Vormerkung übernimmt es).
pub fn execute(ctx: &Ctx, scan: &Scan, sel: &Selection, opts: &Opts, phase: Phase) -> Vec<ItemReport> {
    let now = SystemTime::now();
    let items = sel.expanded();
    let reset = items.contains(&Item::FactoryReset);
    let mut out = Vec::new();
    let runs_here = |i: Item| match phase {
        Phase::Live => !i.needs_restart() && !i.is_setting(),
        Phase::Start => !i.outside_data(),
    };
    let any_here = items.iter().any(|i| runs_here(*i));
    // 1. Datenbank-Sicherung: abschaltbar, erzwungen, wenn die Datenbank gelöscht wird. Beim Zurücksetzen ist die
    // ZIP-Sicherung (vorher im laufenden Betrieb) die Sicherung; sie liegt außerhalb des Datenordners.
    let db_here = items.contains(&Item::Database) && phase == Phase::Start;
    let mut backup_made: Option<PathBuf> = None;
    let mut backup_failed = false;
    if !reset && (db_here || (opts.backup_db && any_here)) {
        if let Some((r, p)) = backup_database(ctx) {
            backup_failed = p.is_none();
            backup_made = p;
            out.push(r);
        }
    }
    let mut done: BTreeSet<PathBuf> = BTreeSet::new();
    for item in Item::ALL {
        if !items.contains(&item) || !runs_here(item) || item == Item::FactoryReset {
            continue;
        }
        let Some(found) = scan.found(item) else { continue };
        if item.optional() && found.is_empty() && !sel.items.contains(&item) {
            continue;
        }
        let mut r = ItemReport::new(item.id());
        match item {
            Item::Database => {
                if backup_failed && !reset {
                    r.errors.push(crate::state::t("Die Sicherung ist fehlgeschlagen – die Datenbank wurde nicht gelöscht.", "The backup failed – the database was not deleted.").to_string());
                } else {
                    delete_files(found.files.iter().filter(|f| done.insert(f.path.clone())), &mut r);
                }
            }
            Item::DbBackups => {
                let keep = if reset { 0 } else { opts.keep_backups };
                let o = Opts { keep_backups: keep, ..opts.clone() };
                // Die Sicherung dieses Laufs bleibt immer (auch bei „alle“).
                let files: Vec<&FileEntry> = selected_files(item, found, &o, now).into_iter().filter(|f| backup_made.as_deref() != Some(f.path.as_path())).collect();
                delete_files(files.into_iter().filter(|f| done.insert(f.path.clone())), &mut r);
            }
            i if i.is_setting() => {
                // Nur in der Startphase (im laufenden Betrieb ändert die Oberfläche die geladenen Einstellungen).
                apply_setting_file(ctx, item, &mut r);
            }
            Item::Autostart | Item::ContextMenu | Item::ClaudeJson => integ::remove(ctx, item, &mut r),
            _ => {
                let o = if reset { Opts { older_than_days: None, ..opts.clone() } } else { opts.clone() };
                let files: Vec<&FileEntry> = selected_files(item, found, &o, now);
                delete_files(files.into_iter().filter(|f| done.insert(f.path.clone())), &mut r);
                for sub in ["clipboard", HOT_EXIT] {
                    prune_empty_dirs(&ctx.data_dir.join(sub));
                }
                if item == Item::RunTemp {
                    let d = ctx.temp_dir.join(RUN_TEMP);
                    if !root_is_link(&d) {
                        prune_empty_dirs(&d);
                        let _ = std::fs::remove_dir(&d);
                    }
                }
            }
        }
        for l in &found.linked {
            let p = l.display().to_string();
            r.note(format!("Nicht berührt: {p} ist ein Link (Symlink/Junction)."), format!("Not touched: {p} is a link (symlink/junction)."));
        }
        if item == Item::Hooks && phase == Phase::Live {
            r.note("Paneldeck legt die Datei beim nächsten Start wieder an, solange es Claude-Sitzungen startet.", "Paneldeck creates the file again at its next start, as long as it starts Claude sessions.");
        }
        out.push(r);
    }
    if reset && phase == Phase::Start {
        // Alles Übrige im Datenordner (außer Sperre, IPC-Datei und anderen Profilen) und leere Ordner.
        let mut r = ItemReport::new(Item::FactoryReset.id());
        let rest: Vec<FileEntry> = data_files(&ctx.data_dir).into_iter().filter(|f| !done.contains(&f.path)).collect();
        delete_files(&rest, &mut r);
        prune_empty_dirs_followed(&ctx.data_dir);
        out.push(r);
    }
    if phase == Phase::Live {
        for p in scan.profiles.iter().filter(|p| sel.profiles.contains(&p.name)) {
            let mut r = ItemReport::new(&format!("profile:{}", p.name));
            delete_profile(p, &mut r);
            if r.ok() {
                integ::remove_profile_autostart(ctx, &p.name, &mut r);
            }
            out.push(r);
        }
    }
    out
}

/// Einstellungs-Eintrag direkt in `settings.json` (Startphase).
fn apply_setting_file(ctx: &Ctx, item: Item, r: &mut ItemReport) {
    let path = ctx.data_dir.join("settings.json");
    if !path.is_file() {
        return;
    }
    if item == Item::SettingsAll {
        match crate::state::reset_settings(&ctx.data_dir) {
            Ok(Some(b)) => {
                let p = b.display().to_string();
                r.files = 1;
                r.note(format!("Sicherung: {p}"), format!("Backup: {p}"));
            }
            Ok(None) => {}
            Err(e) => r.errors.push(format!("{}: {e}", path.display())),
        }
        return;
    }
    let mut s: crate::state::Settings = crate::state::load_json(&path).unwrap_or_default();
    reset_setting(item, &mut s, true);
    match crate::state::write_atomic(&path, serde_json::to_string_pretty(&s).unwrap_or_default().as_bytes()) {
        Ok(()) => r.entries = 1,
        Err(e) => r.errors.push(format!("{}: {e}", path.display())),
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Vormerkung für den Neustart

/// Inhalt von [`PENDING_FILE`].
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Pending {
    pub version: u32,
    /// Kennungen der Einträge (unbekannte werden ignoriert).
    pub items: Vec<String>,
    pub opts: Opts,
    /// Aktiver Sicherungsordner (auch `--backup-dir` gilt so über den Neustart).
    pub backup_dir: Option<PathBuf>,
    /// Ergebnisse des Teils, der schon im laufenden Betrieb lief (Integration, Profile, ZIP).
    pub done: Vec<ItemReport>,
}

/// Derselbe Ordner (absolut, ohne Groß-/Kleinschreibung und Trenner am Ende)?
fn same_dir(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| crate::launch::norm_path(&std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()).to_string_lossy());
    norm(a) == norm(b)
}

/// Für den nächsten Start vormerken.
pub fn schedule(data_dir: &Path, p: &Pending) -> Result<(), String> {
    let text = serde_json::to_string_pretty(p).map_err(|e| e.to_string())?;
    crate::state::write_atomic(&data_dir.join(PENDING_FILE), text.as_bytes()).map_err(|e| e.to_string())
}

/// Beim Start vor dem Laden: eine Vormerkung ausführen (sie wird vorher entfernt, damit ein Absturz keine Schleife
/// ergibt). `backup_dir` = Sicherungsordner dieses Starts (`--backup-dir`, sonst aus den Einstellungen), falls die
/// Vormerkung keinen nennt. `None`, wenn nichts vorgemerkt war.
pub fn apply_pending(data_dir: &Path, backup_dir: impl FnOnce() -> PathBuf) -> Option<Vec<ItemReport>> {
    let marker = data_dir.join(PENDING_FILE);
    let text = std::fs::read_to_string(&marker).ok()?;
    let _ = std::fs::remove_file(&marker);
    let mut out = Vec::new();
    let p: Pending = match serde_json::from_str(crate::state::strip_bom(&text)) {
        Ok(p) => p,
        Err(e) => {
            let mut r = ItemReport::new("pending");
            r.errors.push(format!("{PENDING_FILE}: {e}"));
            return Some(vec![r]);
        }
    };
    out.extend(p.done.iter().cloned());
    let mut sel = Selection::default();
    for id in &p.items {
        match Item::from_id(id) {
            // Integration und andere Profile nie aus einer Datei im Datenordner.
            Some(i) if !i.outside_data() => {
                sel.items.insert(i);
            }
            _ => {
                let mut r = ItemReport::new(id);
                r.errors.push(crate::state::t("Nicht beim Start ausführbar.", "Cannot run at start.").to_string());
                out.push(r);
            }
        }
    }
    // Der Sicherungsordner kommt nie aus der Datei (sie liegt im Datenordner und könnte auf einen fremden Ordner zeigen,
    // dessen Dateien beim Zurücksetzen gelöscht würden): immer der dieses Starts; ein abweichender Wert wird genannt.
    let here = backup_dir();
    if let Some(f) = p.backup_dir.as_ref().filter(|f| !same_dir(f, &here)) {
        let mut r = ItemReport::new("pending");
        let (a, b) = (f.display().to_string(), here.display().to_string());
        r.note(format!("Sicherungsordner der Vormerkung ({a}) ignoriert – benutzt wird {b}."), format!("Backup folder of the scheduled clean-up ({a}) ignored – using {b}."));
        out.push(r);
    }
    let ctx = Ctx { data_dir: data_dir.to_path_buf(), backup_dir: here, base_dir: data_dir.to_path_buf(), ..Default::default() };
    let scan = scan(&ctx);
    out.extend(execute(&ctx, &scan, &sel, &p.opts, Phase::Start));
    Some(out)
}

/// ZIP-Sicherung des Datenordners nach `target_dir` (`paneldeck-backup-<zeit>.zip`, ohne Zugangsdaten, siehe `zip`). Der
/// Zielordner darf nicht im Datenordner liegen (er würde danach gelöscht). Die Datenbank kommt als Kopie hinein.
pub fn zip_backup(data_dir: &Path, target_dir: &Path) -> ItemReport {
    let mut r = ItemReport::new("zip_backup");
    let norm = |p: &Path| crate::launch::norm_path(&std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()).to_string_lossy());
    let (t, d) = (norm(target_dir), norm(data_dir));
    if t.is_empty() || t == d || t.starts_with(&format!("{d}\\")) || t.starts_with(&format!("{d}/")) {
        r.errors.push(crate::state::t("Der Zielordner der ZIP-Sicherung darf nicht im Datenordner liegen.", "The target folder of the ZIP backup must not be inside the data folder.").to_string());
        return r;
    }
    if let Err(e) = std::fs::create_dir_all(target_dir) {
        r.errors.push(format!("{}: {e}", target_dir.display()));
        return r;
    }
    let db = data_dir.join(crate::db::FILE);
    let tmp = std::env::temp_dir().join(format!("paneldeck-cleanup-{}-{}", std::process::id(), stamp()));
    let copy = if db.is_file() {
        match crate::backup::create(&db, &tmp, crate::backup::Kind::PreCleanup) {
            Ok(b) => Some(b.path),
            Err(e) => {
                r.errors.push(e);
                let _ = std::fs::remove_dir_all(&tmp);
                return r;
            }
        }
    } else {
        None
    };
    let out = free_name(target_dir, "paneldeck-backup", ".zip");
    match zip::data_folder_entries(data_dir, copy.as_deref()).and_then(|e| zip::write(&out, e)) {
        Ok((bytes, files)) => {
            r.bytes = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(bytes);
            r.files = files;
            let p = out.display().to_string();
            r.note(format!("Gespeichert: {p} (ohne Zugangsdaten)"), format!("Saved: {p} (without credentials)"));
        }
        Err(e) => r.errors.push(e),
    }
    let _ = std::fs::remove_dir_all(&tmp);
    r
}

/// Ergebnisse mit gleicher Kennung zusammenführen (Teile aus Oberfläche und Hintergrund), Reihenfolge des ersten Auftretens.
pub fn merge_reports(list: Vec<ItemReport>) -> Vec<ItemReport> {
    let mut out: Vec<ItemReport> = Vec::new();
    for r in list {
        match out.iter_mut().find(|x| x.id == r.id) {
            Some(x) => {
                x.bytes += r.bytes;
                x.files += r.files;
                x.entries += r.entries;
                x.errors.extend(r.errors);
                x.notes.extend(r.notes);
                x.restart |= r.restart;
            }
            None => out.push(r),
        }
    }
    out
}

/// Summe eines Berichts.
pub fn report_total(r: &[ItemReport]) -> (u64, u64) {
    r.iter().filter(|x| x.id != "db_backup" && x.id != "zip_backup").fold((0, 0), |(b, f), x| (b + x.bytes, f + x.files))
}
