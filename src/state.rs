//! Einstellungen und gespeicherter Zustand (portabel neben der .exe).

use crate::layout::Node;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Terminal-Profil: Shell, Startbefehl, Umgebung und Darstellung.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub id: u64,
    pub name: String,
    /// Kennung einer erkannten Shell (leer = Standard-Shell)
    pub shell_id: String,
    /// Eigenes Programm (überschreibt `shell_id`) mit Argumenten
    pub program: String,
    pub args: String,
    pub cwd: String,
    /// Befehl, der nach dem Start eingegeben wird
    pub startup: String,
    /// Umgebungsvariablen, eine pro Zeile: NAME=WERT
    pub env: String,
    /// Theme-Name (leer = globales Theme)
    pub theme: String,
    /// Schriftgröße (0 = global)
    pub font_size: f32,
    pub color: Option<[u8; 3]>,
    /// Als Administrator starten (öffnet ein externes Fenster mit UAC-Abfrage)
    pub admin: bool,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            id: 0,
            name: String::new(),
            shell_id: String::new(),
            program: String::new(),
            args: String::new(),
            cwd: String::new(),
            startup: String::new(),
            env: String::new(),
            theme: String::new(),
            font_size: 0.0,
            color: None,
            admin: false,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum Lang {
    De,
    En,
}

impl Lang {
    pub fn t(self, de: &'static str, en: &'static str) -> &'static str {
        match self {
            Lang::De => de,
            Lang::En => en,
        }
    }
}

impl Lang {
    /// Text in landesüblichen Anführungszeichen: „…“ (Deutsch) bzw. “…” (Englisch).
    pub fn q(self, s: &str) -> String {
        match self {
            Lang::De => format!("„{s}“"),
            Lang::En => format!("“{s}”"),
        }
    }
}

/// Globale Sprache für Stellen ohne Zugriff auf die Einstellungen (Hook-/MCP-Server, Git-Hilfen, Brücke).
/// Standard ist Englisch; die Oberfläche schreibt sie nach dem Laden der Einstellungen und bei jeder Änderung.
static LANG: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(1);

pub fn set_lang(l: Lang) {
    LANG.store(if l == Lang::De { 0 } else { 1 }, std::sync::atomic::Ordering::Relaxed);
}

pub fn lang() -> Lang {
    if LANG.load(std::sync::atomic::Ordering::Relaxed) == 0 { Lang::De } else { Lang::En }
}

/// Text in der globalen Sprache.
pub fn t(de: &'static str, en: &'static str) -> &'static str {
    lang().t(de, en)
}

/// Sprache aus `settings.json` des Datenordners übernehmen (für Prozesse ohne Oberfläche, z. B. die MCP-Brücke).
pub fn init_lang_from_disk(dir: &std::path::Path) {
    let l = load_json::<Settings>(&dir.join("settings.json")).map(|s| s.language).unwrap_or(Lang::En);
    set_lang(l);
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub language: Lang,
    pub theme: String,
    pub font_size: f32,
    pub default_shell: String,
    /// Claude-spezifische Bedienelemente anzeigen.
    pub claude_mode: bool,
    /// Beim Wiederherstellen Claude-Sitzungen mit `claude --continue` fortsetzen.
    pub claude_resume: bool,
    pub scrollback_lines: usize,
    pub confirm_close: bool,
    pub sidebar_width: f32,
    pub notify_toast: bool,
    pub notify_sound: bool,
    pub notify_flash: bool,
    /// Schließen-Button minimiert das Fenster (Tray-Symbol bleibt) statt die App zu beenden.
    pub close_to_tray: bool,
    /// Minimieren versteckt das Fenster im Tray (ohne Taskleistenbutton).
    pub minimize_to_tray: bool,
    /// Fensterdeckkraft 0.3–1.0 (Terminalhintergrund und Seitenleiste).
    pub opacity: f32,
    /// Weiche Animationen (aus = sofortige Zustandswechsel).
    pub animations: bool,
    /// Terminalschrift: "Cascadia Mono" (eingebettet), "Hack", "Consolas", "Lucida Console", "Courier New".
    pub terminal_font: String,
    /// Abweichende Tastenkürzel (Kennung → "Ctrl+Shift+T").
    pub keys: std::collections::BTreeMap<String, String>,
    /// Preise für die Kostenschätzung.
    pub prices: crate::usage::Prices,
    /// Shell-Integration (Befehlsmarken, Ordner-Tracking) per Startargument/Umgebung.
    pub profiles: Vec<Profile>,
    pub shell_integration: bool,
    /// Benachrichtigung, wenn ein Befehl länger als so viele Sekunden lief (0 = aus).
    pub cmd_notify_secs: u32,
    /// Skalierung der gesamten Oberfläche (0.7–2.5)
    pub ui_scale: f32,
    /// Tagesbudget in USD (0 = aus): Hinweis, sobald die geschätzten Tageskosten es überschreiten.
    pub daily_budget: f64,
    /// Budget je Sitzung in USD (0 = aus): Hinweis, sobald eine einzelne Sitzung es überschreitet (einmal je Sitzung).
    pub session_budget: f64,
    /// Agenten-Schnittstelle (MCP): Aus / Nur lesen / Steuern. Standard: Aus.
    pub mcp_level: crate::mcp::McpLevel,
    /// Agenten dürfen auch Sitzungen schließen, die sie nicht selbst angelegt haben.
    pub mcp_close_any: bool,
    /// Sprache des Systemprompts für Agenten (`initialize`), unabhängig von der Sprache der Oberfläche. Standard: Englisch;
    /// ein ungültiger Wert ergibt Englisch (die Datei bleibt lesbar).
    #[serde(deserialize_with = "crate::mcp::sysprompt::de_lang")]
    pub mcp_prompt_lang: Lang,
    /// Automatische Datenbank-Sicherung: Intervall in Stunden (0 = aus).
    pub backup_interval_hours: u32,
    /// Wie viele automatische Sicherungen behalten werden (zusätzlich zur Datenbank selbst).
    pub backup_keep: u32,
    /// Sicherungsordner (leer = `backups/` im Datenordner).
    pub backup_dir: String,
    /// Wie viele Änderungen der Todo-Liste sich rückgängig machen lassen (3, 5, 10, 25, 50, 100).
    pub undo_steps: u32,
}

/// Auswahl für `Settings::undo_steps`.
pub const UNDO_STEP_CHOICES: [u32; 6] = [3, 5, 10, 25, 50, 100];

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: Lang::En,
            theme: "One Dark".into(),
            font_size: 14.0,
            default_shell: String::new(),
            claude_mode: true,
            claude_resume: true,
            scrollback_lines: 10_000,
            confirm_close: true,
            sidebar_width: 240.0,
            notify_toast: true,
            notify_sound: true,
            notify_flash: true,
            close_to_tray: false,
            minimize_to_tray: false,
            opacity: 1.0,
            animations: true,
            terminal_font: "Cascadia Mono".into(),
            keys: Default::default(),
            prices: Default::default(),
            profiles: Vec::new(),
            shell_integration: true,
            cmd_notify_secs: 30,
            ui_scale: 1.0,
            daily_budget: 0.0,
            session_budget: 0.0,
            mcp_level: crate::mcp::McpLevel::Off,
            mcp_close_any: false,
            mcp_prompt_lang: Lang::En,
            backup_interval_hours: 24,
            backup_keep: 2,
            backup_dir: String::new(),
            undo_steps: 25,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct Worktree {
    pub repo: String,
    pub path: String,
    pub branch: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionMeta {
    pub id: u64,
    pub name: String,
    /// `true`, wenn der Name manuell vergeben wurde; sonst folgt er dem Terminal-Titel.
    pub custom_name: bool,
    pub shell_id: String,
    pub cwd: String,
    pub color: [u8; 3],
    pub group: String,
    pub claude: bool,
    /// Zusätzliche claude-Optionen (Modell, Permission-Mode).
    pub claude_args: String,
    /// Konversations-ID der Claude-Sitzung (aus den Hooks).
    pub claude_session: String,
    /// Es wurde bereits ein Prompt gesendet, also gibt es etwas zum Fortsetzen.
    pub claude_started: bool,
    pub notes: String,
    /// Pfad des Claude-Transkripts (für Token/Kosten).
    pub transcript: String,
    /// Wartende Prompts (werden nacheinander gesendet, sobald Claude "bereit" meldet).
    pub queue: Vec<String>,
    pub queue_paused: bool,
    /// Profil (0 = keins)
    pub profile: u64,
    /// Eigener Git-Worktree dieser Sitzung (falls angelegt).
    pub worktree: Option<Worktree>,
    /// Projekt (0 = noch keinem zugeordnet; wird beim Laden dem ersten Projekt zugewiesen).
    pub project: u64,
    pub tags: Vec<String>,
}

impl Default for SessionMeta {
    fn default() -> Self {
        Self {
            id: 0,
            name: String::new(),
            custom_name: false,
            shell_id: String::new(),
            cwd: String::new(),
            color: [0x88, 0x88, 0x88],
            group: String::new(),
            claude: false,
            claude_args: String::new(),
            claude_session: String::new(),
            claude_started: false,
            notes: String::new(),
            transcript: String::new(),
            queue: Vec::new(),
            queue_paused: false,
            profile: 0,
            worktree: None,
            project: 0,
            tags: Vec::new(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Project {
    pub id: u64,
    pub name: String,
    pub color: [u8; 3],
    /// Standardordner für neue Sitzungen dieses Projekts.
    pub folder: String,
    /// Standardprofil für neue Sitzungen dieses Projekts (0 = keins)
    pub profile: u64,
    pub layout: Option<Node>,
    pub focused: u64,
    /// Seitenleiste als Ordnerbaum statt als Liste anzeigen.
    pub tree_view: bool,
    /// Ordner dieses Projekts (Pfade mit `/`, auch leere).
    pub folders: Vec<Folder>,
}

impl Default for Project {
    fn default() -> Self {
        Self { id: 0, name: String::new(), color: [0x61, 0xaf, 0xef], folder: String::new(), profile: 0, layout: None, focused: 0, tree_view: false, folders: Vec::new() }
    }
}

/// Ein Ordner im Sitzungsbaum. Sitzungen verweisen über `SessionMeta::group` (= Pfad) darauf.
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Folder {
    pub path: String,
    pub color: Option<[u8; 3]>,
    /// Arbeitsordner-Vorgabe für neue Sitzungen (leer = erbt vom übergeordneten Ordner/Projekt).
    pub cwd: String,
    /// Profil-Vorgabe (0 = erbt).
    pub profile: u64,
    pub collapsed: bool,
}

pub const MAX_FOLDER_DEPTH: usize = 8;

/// Bereinigt einen Ordnerpfad: `\` → `/`, leere Segmente entfallen, Segmente werden gekürzt, Tiefe begrenzt.
pub fn clean_folder_path(s: &str) -> String {
    s.replace('\\', "/")
        .split('/')
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .take(MAX_FOLDER_DEPTH)
        .map(|p| p.chars().take(60).collect::<String>())
        .collect::<Vec<_>>()
        .join("/")
}

pub fn folder_parent(path: &str) -> &str {
    path.rsplit_once('/').map(|(p, _)| p).unwrap_or("")
}

pub fn folder_leaf(path: &str) -> &str {
    path.rsplit_once('/').map(|(_, l)| l).unwrap_or(path)
}

/// `path` liegt in (oder ist) `ancestor`.
pub fn folder_within(path: &str, ancestor: &str) -> bool {
    ancestor.is_empty() || path == ancestor || path.strip_prefix(ancestor).is_some_and(|r| r.starts_with('/'))
}

/// Pfad nach dem Löschen von `deleted`: Inhalt rückt eine Ebene nach oben.
pub fn lift_path(p: &str, deleted: &str) -> String {
    if !folder_within(p, deleted) {
        return p.to_string();
    }
    let rest = p[deleted.len()..].trim_start_matches('/');
    let parent = folder_parent(deleted);
    match (parent.is_empty(), rest.is_empty()) {
        (true, _) => rest.to_string(),
        (false, true) => parent.to_string(),
        (false, false) => format!("{parent}/{rest}"),
    }
}

impl Project {
    /// Legt den Ordner samt aller übergeordneten an.
    pub fn ensure_folder(&mut self, path: &str) {
        let mut cur = String::new();
        for seg in path.split('/').filter(|s| !s.is_empty()) {
            if !cur.is_empty() {
                cur.push('/');
            }
            cur.push_str(seg);
            if !self.folders.iter().any(|f| f.path == cur) {
                self.folders.push(Folder { path: cur.clone(), ..Folder::default() });
            }
        }
    }

    pub fn folder(&self, path: &str) -> Option<&Folder> {
        self.folders.iter().find(|f| f.path == path)
    }

    /// Nächste gesetzte Vorgabe entlang der Pfadkette (Ordner → Eltern), über `pick`.
    pub fn inherited<T>(&self, path: &str, pick: impl Fn(&Folder) -> Option<T>) -> Option<T> {
        let mut cur = path;
        loop {
            if let Some(v) = self.folder(cur).and_then(&pick) {
                return Some(v);
            }
            if cur.is_empty() {
                return None;
            }
            cur = folder_parent(cur);
        }
    }

    /// Benennt einen Ordner samt Unterordnern um (`old` → `new`). Gibt `false` zurück, wenn `new` schon existiert oder in sich selbst liegt.
    pub fn rename_folder(&mut self, old: &str, new: &str) -> bool {
        if old == new || new.is_empty() || folder_within(new, old) || self.folder(new).is_some() {
            return false;
        }
        for f in &mut self.folders {
            if folder_within(&f.path, old) {
                f.path = format!("{}{}", new, &f.path[old.len()..]);
            }
        }
        self.ensure_folder(new);
        true
    }

    /// Löscht den Ordner mit Unterordnern aus der Liste.
    #[cfg(test)]
    pub fn remove_folder(&mut self, path: &str) {
        self.folders.retain(|f| !folder_within(&f.path, path));
    }

    /// Löscht nur den Ordner selbst; Unterordner rücken eine Ebene hoch.
    pub fn delete_folder_lift(&mut self, path: &str) {
        self.folders.retain(|f| f.path != path);
        for f in &mut self.folders {
            f.path = lift_path(&f.path, path);
        }
        let mut seen = std::collections::HashSet::new();
        self.folders.retain(|f| !f.path.is_empty() && seen.insert(f.path.clone()));
    }
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SavedState {
    pub sessions: Vec<SessionMeta>,
    /// Alter Einzel-Layout-Stand (vor Projekten); wird beim Laden ins erste Projekt übernommen.
    pub layout: Option<Node>,
    pub focused: u64,
    pub next_id: u64,
    pub projects: Vec<Project>,
    pub active_project: u64,
    /// In eigenen Fenstern abgedockte Sitzungen.
    pub detached: Vec<u64>,
    pub composer_draft: String,
}

/// Datenordner: `paneldeck-data` neben der .exe, sonst %APPDATA%/Paneldeck.
pub fn data_dir() -> PathBuf {
    let try_dir = |p: PathBuf| -> Option<PathBuf> {
        std::fs::create_dir_all(&p).ok()?;
        let probe = p.join(".probe");
        std::fs::write(&probe, b"x").ok()?;
        let _ = std::fs::remove_file(probe);
        Some(p)
    };
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            if let Some(p) = try_dir(dir.join("paneldeck-data")) {
                return p;
            }
        }
    }
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    try_dir(base.join("Paneldeck")).unwrap_or_else(|| PathBuf::from("."))
}

/// Entfernt ein UTF-8-BOM am Textanfang. Windows-Editoren (Notepad, PowerShell 5.1 `-Encoding UTF8`) schreiben es,
/// `serde_json` lehnt es ab – die Datei galt dann als beschädigt und wurde durch Standardwerte ersetzt.
pub fn strip_bom(s: &str) -> &str {
    s.strip_prefix('\u{feff}').unwrap_or(s)
}

pub fn load_json<T: for<'de> Deserialize<'de>>(path: &std::path::Path) -> Option<T> {
    let s = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str(strip_bom(&s)) {
        Ok(v) => Some(v),
        Err(_) => {
            backup_corrupt(path);
            None
        }
    }
}

/// Atomar schreiben (erst temporäre Datei, dann umbenennen).
pub fn save_json<T: Serialize>(path: &std::path::Path, value: &T) {
    if let Ok(s) = serde_json::to_string_pretty(value) {
        let _ = write_atomic(path, s.as_bytes());
    }
}

/// Schreibt über eine temporäre Datei daneben und benennt sie dann um – ein Absturz mitten im
/// Schreiben hinterlässt nie eine halb geschriebene Zieldatei.
pub fn write_atomic(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".tmp");
    let tmp = path.with_file_name(name);
    if let Err(e) = std::fs::write(&tmp, data) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Defekte Datei nicht stillschweigend überschreiben, sondern mit Zeitstempel sichern (die letzten 5 bleiben).
pub fn backup_corrupt(path: &std::path::Path) -> Option<PathBuf> {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let stem = path.file_stem()?.to_string_lossy().to_string();
    let target = path.with_file_name(format!("{stem}.defekt-{secs}.json"));
    std::fs::copy(path, &target).ok()?;
    // Alte Sicherungen aufräumen
    if let Some(dir) = path.parent() {
        let mut old: Vec<PathBuf> = std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&format!("{stem}.defekt-"))))
            .collect();
        old.sort();
        while old.len() > 5 {
            let _ = std::fs::remove_file(old.remove(0));
        }
    }
    Some(target)
}

/// Zustand tolerant laden: eine einzelne ungültige Sitzung verwirft nicht alles. Gibt (Zustand, Hinweistext) zurück.
pub fn load_saved(path: &std::path::Path) -> (SavedState, Option<String>) {
    let Ok(text) = std::fs::read_to_string(path) else { return (SavedState::default(), None) };
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(strip_bom(&text)) else {
        let b = backup_corrupt(path);
        let bk = b.map(|p| p.display().to_string()).unwrap_or_default();
        let msg = if lang() == Lang::De { format!("Zustandsdatei war beschädigt – Sicherung: {bk}") } else { format!("State file was corrupt – backup: {bk}") };
        return (SavedState::default(), Some(msg));
    };
    let sessions_v = v.as_object_mut().and_then(|o| o.remove("sessions"));
    let mut note: Option<String> = None;
    let mut st = match serde_json::from_value::<SavedState>(v) {
        Ok(s) => s,
        Err(_) => {
            let b = backup_corrupt(path);
            let bk = b.map(|p| p.display().to_string()).unwrap_or_default();
            note = Some(if lang() == Lang::De { format!("Teile der Zustandsdatei waren ungültig (Sicherung: {bk})") } else { format!("Parts of the state file were invalid (backup: {bk})") });
            SavedState::default()
        }
    };
    let (mut skipped, mut seen) = (0usize, std::collections::HashSet::new());
    if let Some(serde_json::Value::Array(items)) = sessions_v {
        for item in items {
            match serde_json::from_value::<SessionMeta>(item) {
                // Doppelte oder unsinnige IDs überspringen (sonst würde die zweite die erste verdrängen).
                Ok(m) if m.id > 0 && m.id < 1_000_000_000_000 && seen.insert(m.id) => st.sessions.push(m),
                _ => skipped += 1,
            }
        }
    }
    if skipped > 0 {
        let b = backup_corrupt(path);
        let bk = b.map(|p| p.display().to_string()).unwrap_or_default();
        note = Some(if lang() == Lang::De { format!("{skipped} ungültige Sitzung(en) übersprungen (Sicherung: {bk})") } else { format!("{skipped} invalid session(s) skipped (backup: {bk})") });
    }
    (st, note)
}

fn clamp_f32(v: f32, lo: f32, hi: f32, default: f32) -> f32 {
    if v.is_finite() { v.clamp(lo, hi) } else { default }
}

impl Settings {
    /// Unsinnige Werte aus einer von Hand bearbeiteten Datei auf gültige Bereiche begrenzen (verhindert Abstürze beim Start).
    pub fn sanitize(&mut self) {
        self.font_size = clamp_f32(self.font_size, 8.0, 40.0, 14.0);
        self.opacity = clamp_f32(self.opacity, 0.3, 1.0, 1.0);
        self.sidebar_width = clamp_f32(self.sidebar_width, 210.0, 460.0, 240.0);
        if !self.daily_budget.is_finite() || self.daily_budget < 0.0 {
            self.daily_budget = 0.0;
        }
        if !self.session_budget.is_finite() || self.session_budget < 0.0 {
            self.session_budget = 0.0;
        }
        self.ui_scale = clamp_f32(self.ui_scale, 0.7, 2.5, 1.0);
        self.scrollback_lines = self.scrollback_lines.clamp(1000, 100_000);
        self.cmd_notify_secs = self.cmd_notify_secs.min(3600);
        self.backup_interval_hours = self.backup_interval_hours.min(720);
        self.backup_keep = self.backup_keep.clamp(1, 50);
        // auf den nächsten erlaubten Wert einrasten (0 aus einer alten/handbearbeiteten Datei → Standard 25)
        self.undo_steps = if self.undo_steps == 0 { 25 } else { *UNDO_STEP_CHOICES.iter().min_by_key(|c| c.abs_diff(self.undo_steps)).unwrap_or(&25) };
        for p in &mut self.profiles {
            p.font_size = if p.font_size.is_finite() && p.font_size >= 6.0 { p.font_size.min(48.0) } else { 0.0 };
        }
        let fix = |v: &mut f64, d: f64| if !v.is_finite() || *v < 0.0 { *v = d };
        let d = crate::usage::Prices::default();
        for (a, b) in [(&mut self.prices.opus, d.opus), (&mut self.prices.sonnet, d.sonnet), (&mut self.prices.haiku, d.haiku)] {
            fix(&mut a[0], b[0]);
            fix(&mut a[1], b[1]);
        }
        fix(&mut self.prices.cache_read, d.cache_read);
        fix(&mut self.prices.cache_write, d.cache_write);
    }
}

#[cfg(test)]
mod folder_tests {
    use super::*;
    /// Die Standardsprache ist Englisch (Issue #11); Deutsch nur auf ausdrücklichen Wunsch.
    /// Setzt die globale Sprache absichtlich nie auf Deutsch (andere Tests laufen parallel).
    #[test]
    fn default_language_is_english() {
        assert_eq!(Settings::default().language, Lang::En);
        assert_eq!(lang(), Lang::En);
        assert_eq!(t("Hallo", "Hello"), "Hello");
        assert_eq!(Lang::En.q("x"), "\u{201c}x\u{201d}");
        assert_eq!(Lang::De.q("x"), "\u{201e}x\u{201c}");
    }
    #[test]
    fn write_atomic_replaces_and_leaves_no_temp() {
        let d = std::env::temp_dir().join(format!("wa-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("a.json");
        write_atomic(&f, b"eins").unwrap();
        write_atomic(&f, b"zwei").unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"zwei");
        assert!(!d.join("a.json.tmp").exists());
        let _ = std::fs::remove_dir_all(&d);
    }
    #[test]
    fn paths() {
        assert_eq!(clean_folder_path(" a / b\\ c //"), "a/b/c");
        assert_eq!(clean_folder_path("///"), "");
        assert_eq!(folder_parent("a/b/c"), "a/b");
        assert_eq!(folder_parent("a"), "");
        assert_eq!(folder_leaf("a/b"), "b");
        assert!(folder_within("a/b", "a"));
        assert!(!folder_within("ab", "a"));
        assert!(folder_within("x", ""));
    }
    #[test]
    fn ensure_rename_inherit() {
        let mut p = Project::default();
        p.ensure_folder("a/b/c");
        assert_eq!(p.folders.len(), 3);
        p.folders[0].cwd = "C:/x".into();
        assert_eq!(p.inherited("a/b/c", |f| Some(f.cwd.clone()).filter(|c| !c.is_empty())).as_deref(), Some("C:/x"));
        assert!(p.rename_folder("a/b", "a/z"));
        assert!(p.folder("a/z/c").is_some() && p.folder("a/b").is_none());
        assert!(!p.rename_folder("a", "a/q"));
        p.remove_folder("a/z");
        assert_eq!(p.folders.len(), 1);
    }
    #[test]
    fn lift() {
        assert_eq!(lift_path("a/b/c", "a/b"), "a/c");
        assert_eq!(lift_path("a/b", "a/b"), "a");
        assert_eq!(lift_path("a", "a"), "");
        assert_eq!(lift_path("a/c", "a"), "c");
        assert_eq!(lift_path("x", "a"), "x");
        let mut p = Project::default();
        p.ensure_folder("a/b/c");
        p.delete_folder_lift("a/b");
        let v: Vec<_> = p.folders.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(v, ["a", "a/c"]);
    }
    #[test]
    fn json_with_bom_is_not_corrupt() {
        // Notepad / PowerShell 5.1 (-Encoding UTF8) schreiben ein BOM; die Datei darf nicht als defekt gelten.
        let dir = std::env::temp_dir().join(format!("bom-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x.json");
        std::fs::write(&p, "\u{feff}{\"a\":1}").unwrap();
        let v: Option<serde_json::Value> = load_json(&p);
        assert_eq!(v.unwrap()["a"], 1);
        assert!(p.exists(), "Datei darf nicht als defekt weggesichert werden");
        assert_eq!(strip_bom("\u{feff}x"), "x");
        assert_eq!(strip_bom("x"), "x");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_steps_snap_to_allowed_values() {
        let mut s = Settings::default();
        assert_eq!(s.undo_steps, 25);
        for (given, want) in [(0, 25), (1, 3), (4, 3), (6, 5), (9, 10), (25, 25), (40, 50), (74, 50), (76, 100), (5000, 100)] {
            s.undo_steps = given;
            s.sanitize();
            assert_eq!(s.undo_steps, want, "{given}");
        }
        // fehlt der Wert in einer alten settings.json, gilt der Standard
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.undo_steps, 25);
        assert!(UNDO_STEP_CHOICES.contains(&s.undo_steps));
    }
}
