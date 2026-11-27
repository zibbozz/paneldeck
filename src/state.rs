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
    /// Shell in WSL (#57): leer = Login-Shell der Distribution (automatisch), sonst ein Pfad aus deren `/etc/shells`
    /// (z. B. `/usr/bin/zsh`); startet als `wsl.exe … -e <pfad> -l`. Nur für Profile mit einer WSL-Shell ohne eigenes Programm.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub login_shell: String,
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
            login_shell: String::new(),
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

// `--lang` gilt nur für diesen Start. Regel: Alles, was gespeichert wird (settings.json, Standard-Prompts beim
// ersten Start, Texte im Explorer-Kontextmenü …), nutzt die GESPEICHERTE Sprache (`persisted_lang`), nur die
// Anzeige nutzt die Startsprache. Abgelegt wird das Paar (Startsprache, gespeicherte Sprache) außerhalb von
// `Settings` (nicht serialisiert) und threadlokal: Einstellungen werden nur im UI-Thread gespeichert, und Tests
// (parallel) beeinflussen sich so nicht. Solange `Settings::language` gleich der Startsprache ist, schreibt
// `ser_language` die gespeicherte Sprache. Wählt der Nutzer in den Einstellungen selbst eine Sprache, endet die
// Überschreibung (`clear_lang_override`) und seine Wahl wird gespeichert.
thread_local! {
    static LANG_OVERRIDE: std::cell::Cell<Option<(Lang, Lang)>> = const { std::cell::Cell::new(None) };
}

pub fn set_lang_override(start: Lang, persisted: Lang) {
    LANG_OVERRIDE.with(|o| o.set(Some((start, persisted))));
}

pub fn clear_lang_override() {
    LANG_OVERRIDE.with(|o| o.set(None));
}

/// Sprache für Gespeichertes, ausgehend von der aktuell angezeigten Sprache `current`.
pub fn persisted_lang(current: Lang) -> Lang {
    persisted_language(current, LANG_OVERRIDE.with(|o| o.get()))
}

/// Sprache, die in `settings.json` landet.
pub fn persisted_language(l: Lang, over: Option<(Lang, Lang)>) -> Lang {
    match over {
        Some((start, persisted)) if start == l => persisted,
        _ => l,
    }
}

fn ser_language<S: serde::Serializer>(l: &Lang, s: S) -> Result<S::Ok, S::Error> {
    persisted_lang(*l).serialize(s)
}

/// Sprache aus `settings.json` lesen, ohne etwas zu verändern (keine Sicherung defekter Dateien).
pub fn lang_from_disk(dir: &std::path::Path) -> Option<Lang> {
    let s = std::fs::read_to_string(dir.join("settings.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(strip_bom(&s)).ok()?;
    serde_json::from_value(v.get("language")?.clone()).ok()
}

/// Sprache aus `settings.json` des Datenordners übernehmen (für Prozesse ohne Oberfläche, z. B. die MCP-Brücke).
pub fn init_lang_from_disk(dir: &std::path::Path) {
    let l = load_json::<Settings>(&dir.join("settings.json")).map(|s| s.language).unwrap_or(Lang::En);
    set_lang(l);
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Mit `--lang` überschreibt der Start die Sprache nur im Speicher; gespeichert wird die vorherige (`ser_language`).
    #[serde(serialize_with = "ser_language")]
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
    /// „Transparenz reduzieren“ (#387, ab 3.2.0): alle Flächen deckend, kein eigener Hintergrund des Themes. Standard aus;
    /// ein falscher Typ gilt als aus.
    #[serde(deserialize_with = "lenient_false")]
    pub reduce_transparency: bool,
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
    /// Agenten dürfen Umgebungsvariablen setzen (#384, 3.2.0): nur in Sitzungen, die sie selbst anlegen (`create_session`
    /// mit `env`), nie in bestehenden, nie im laufenden Terminal. Standard: aus.
    pub mcp_session_env: bool,
    /// Erlaubte Namensmuster für Variablen von Agenten (#384, z. B. `APP_*`); leer = alle außer der festen Sperrliste.
    pub mcp_env_allow: Vec<String>,
    /// Zugriff der Agenten auf den Bereich „Allgemein“ (alles ohne Projekt, #386). Fehlt = wie global, für alle.
    pub agent_general: crate::access::AgentAccess,
    /// Agenten dürfen Ticket-Projekte anlegen (#386, Standard aus; nur mit Steuern im Heimatprojekt).
    pub agent_create_projects: bool,
    /// Agenten dürfen Tickets und eigene Kommentare in den Papierkorb legen (#386, Standard aus; nur mit Steuern).
    pub agent_delete: bool,
    /// Neue Todo-Karten sind privat (#386, Standard aus).
    pub todo_new_private: bool,
    /// Systemnotizen (von Agenten oder Paneldeck angelegte Karten/Tickets) in den Listen zeigen (#386, Standard an).
    pub show_system_notes: bool,
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
    /// Automatisch nach Updates suchen (beim Start, höchstens einmal am Tag). Standard: aus.
    pub update_auto_check: bool,
    /// Zeitpunkt der letzten erfolgreichen Update-Prüfung (Unix-Sekunden, 0 = noch nie).
    pub update_last_check: i64,
    /// Vom Nutzer übersprungene Version (`X.Y.Z`, leer = keine); die automatische Prüfung bietet sie nicht mehr an.
    pub update_skip_version: String,
    /// LLM-Server für Chats (seit 2.2.0, `llm::config`). Unlesbare Einträge bleiben unverändert erhalten.
    #[serde(skip_serializing_if = "crate::llm::config::LlmServers::is_blank")]
    pub llm_servers: crate::llm::config::LlmServers,
    /// Agenten-Werkzeuge der Chats (seit 2.7.0, #338): sensible Dateien, Zeitlimit für Befehle, Web. Fehlt in älteren
    /// Dateien (dann Standard); unlesbare Felder ergeben ihren Standard.
    pub agent_tools: crate::chat::agent::AgentToolSettings,
    /// Zuletzt im Dialog „Neuer Chat“ gewähltes Modell (Vorauswahl beim nächsten Mal).
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient_chat_meta")]
    pub last_chat_model: Option<ChatMeta>,
    /// Mausrad in Vollbildprogrammen (alternativer Bildschirm, Programm ohne Maus-Modus), seit 2.2.0.
    /// Fehlt in älteren Dateien (dann Standard); ein unbekannter Wert ergibt ebenfalls den Standard.
    #[serde(deserialize_with = "AltScreenWheel::lenient")]
    pub alt_screen_wheel: AltScreenWheel,
    /// Sitzungen merken (seit 2.3.0, Standard an): Terminals, Chats, Projekte und Ordner beim nächsten Start
    /// wiederherstellen. Aus: jeder Start ist frisch, Chatverläufe werden beim Beenden gelöscht (`app::fresh_start`).
    /// Fehlt in älteren Dateien (dann an = bisheriges Verhalten); ein falscher Typ ergibt ebenfalls an.
    #[serde(deserialize_with = "lenient_true")]
    pub remember_sessions: bool,
    /// Umschalt+Enter = neue Zeile in PowerShell (seit 2.3.0, Standard aus): sendet in PowerShell-Sitzungen ein echtes
    /// Umschalt+Enter (win32-input-mode), das PSReadLine als „AddLine“ versteht. Aus = ESC CR wie bisher (Claude Code und
    /// andere Programme mit VT-Eingabe behalten Umschalt+Enter). Fehlt in älteren Dateien (dann aus).
    #[serde(deserialize_with = "lenient_false")]
    pub shift_enter_newline_powershell: bool,
    /// Farbpaletten (seit 2.3.0, `palette`): gemeinsam oder je Bereich. Fehlt in älteren Dateien (dann die Vorgaben);
    /// unveränderte Vorgaben werden nicht geschrieben, unlesbare Daten ergeben die Vorgaben.
    #[serde(skip_serializing_if = "crate::palette::Palettes::is_default", deserialize_with = "crate::palette::Palettes::lenient")]
    pub palette: crate::palette::Palettes,
    /// Programme dürfen per OSC 52 in die Zwischenablage kopieren (seit 2.3.0, #181, Standard an). Lesen der
    /// Zwischenablage durch Programme gibt es nie. Fehlt in älteren Dateien (dann an).
    #[serde(deserialize_with = "lenient_true")]
    pub osc52_copy: bool,
    /// Vor dem Einfügen mehrerer Zeilen nachfragen, wenn das Programm kein Bracketed Paste nutzt (seit 2.3.0, #183,
    /// Standard an wie in Windows Terminal). Fehlt in älteren Dateien (dann an).
    #[serde(deserialize_with = "lenient_true")]
    pub warn_multiline_paste: bool,
    /// Standard-Startoptionen für neue Claude-Sitzungen (seit 2.3.0; leeres Feld = Standard von Claude Code). Gelten für
    /// Sitzungen aus der Oberfläche und über MCP, wenn nichts angegeben ist – `bypassPermissions` nie für Agenten.
    /// Nie `bypassPermissions` und nur gültige Namen (beim Schreiben bereinigt, beim Laden verworfen, siehe `sanitize`).
    #[serde(skip_serializing_if = "crate::claude_opts::ClaudeOpts::is_empty", serialize_with = "crate::claude_opts::ser_user_opts")]
    pub claude_defaults: crate::claude_opts::ClaudeOpts,
    /// Zuletzt im Dialog „Neue Sitzung“ gewählte Claude-Optionen (Vorauswahl beim nächsten Mal; `None` = Standards).
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "crate::claude_opts::ser_user_opts_opt")]
    pub last_claude_opts: Option<crate::claude_opts::ClaudeOpts>,
    /// Nur im Speicher: beim Laden stand `bypassPermissions` als Standard in der Datei und wurde verworfen
    /// (Hinweis unter *Einstellungen → Claude*).
    #[serde(skip)]
    pub claude_bypass_dropped: bool,
    /// Mitgelieferten Konsolen-Host (neuere ConPTY: `conpty.dll` + `OpenConsole.exe` neben `paneldeck.exe`) verwenden,
    /// seit 2.3.0, Standard an. Aus oder Dateien fehlen: ConPTY von Windows. Wirkt für neue Sitzungen (`conpty`).
    /// Fehlt in älteren Dateien (dann an); ein anderer Wert als `false` ergibt ebenfalls an.
    #[serde(deserialize_with = "lenient_true")]
    pub bundled_conpty: bool,
    /// Zuletzt im Ordner-Auswahlfeld benutzte WSL-Ordner je Distribution (seit 2.3.0, #172), neueste zuerst,
    /// begrenzt (`wsl::RECENT_MAX`). Fehlt in älteren Dateien; Unlesbares/Ungültiges wird beim Laden verworfen.
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty", deserialize_with = "lenient_wsl_recent")]
    pub wsl_recent: std::collections::BTreeMap<String, Vec<String>>,
    /// Umgebung von Paneldeck erben (seit 2.3.0, #175, Standard aus): Terminals starten mit der Umgebung des
    /// Paneldeck-Prozesses statt mit der frisch aus der Registry gelesenen (portable-pty: Prozessumgebung, darüber System-
    /// und Benutzervariablen aus der Registry – so wirkt `setx` sofort). An: genau die Prozessumgebung, z. B. für einen Start
    /// aus einer Shell mit eigener Umgebung. Paneldecks eigene Variablen (`PANELDECK_SESSION` …) und die Variablen des Profils
    /// gelten in beiden Fällen. Fehlt in älteren Dateien (dann aus = bisheriges Verhalten).
    #[serde(deserialize_with = "lenient_false")]
    pub inherit_env: bool,
    /// Benachrichtigungen von Programmen (OSC 9 / OSC 777;notify) anzeigen – nur für nicht angesehene Sitzungen, über die
    /// üblichen Wege (Toast/Blinken laut `notify_*`), begrenzt (seit 2.3.0, #183, Standard an). Fehlt in älteren Dateien (an).
    #[serde(deserialize_with = "lenient_true")]
    pub program_notify: bool,
    /// Glocke (BEL) einer nicht angesehenen Sitzung: die Markierung in der Seitenleiste gibt es immer; mit dieser Einstellung
    /// zusätzlich Toast/Blinken bzw. Hinweis wie bei Benachrichtigungen (seit 2.3.0, #183, Standard aus – Entscheidung des
    /// Nutzers). Fehlt in älteren Dateien (dann aus).
    #[serde(deserialize_with = "lenient_false")]
    pub bell_toast: bool,
    /// Fortschritt von Programmen (OSC 9;4) in Seitenleiste und Taskleiste zeigen (seit 2.3.0, #183, Standard an).
    #[serde(deserialize_with = "lenient_true")]
    pub progress_indicator: bool,
    /// Blinkender Cursor, wenn das Programm ihn anfordert (DECSCUSR 1/3/5, `CSI ? 12 h`); nur im fokussierten Terminal
    /// (seit 2.3.0, #183, Standard an).
    #[serde(deserialize_with = "lenient_true")]
    pub cursor_blink: bool,
    /// Abgeschaltete Regeln der Design-Prüfung im Theme-Editor (seit 2.6.0, #309; Standard: alle an). Werte: `contrast`,
    /// `distinct`, `status`, `terminal`, `harmony`; unbekannte bleiben erhalten.
    #[serde(deserialize_with = "lenient_strings")]
    pub theme_checks_off: Vec<String>,
    /// Schalter für die Teile des Master-Prompts der Chats (seit 2.7.0, #341; Standard: alle an, dann fehlt das Feld).
    /// Je Modell abweichend in `llm_servers[].models[].prompt_parts`. Tolerant: nur ein echtes `false` schaltet aus.
    #[serde(skip_serializing_if = "crate::llm::prompt_parts::PromptParts::is_default")]
    pub chat_prompt_parts: crate::llm::prompt_parts::PromptParts,
    /// Kompaktieren der Chats (seit 3.1.0, #359): `auto` (Standard aus), `threshold_percent` (80), Sicherheitsfenster
    /// `window_percent` (15) und `window_min` (4096). Fehlt das Feld (Standard), gilt alles wie bisher; Unlesbares ergibt
    /// je Wert den Standard, Zahlen werden auf ihre Grenzen gesetzt.
    #[serde(skip_serializing_if = "crate::chat::compact::CompactSettings::is_default")]
    pub chat_compact: crate::chat::compact::CompactSettings,
    /// Editor-Reiter (seit 3.0.0, #54): Schrift, große Dateien, automatisch speichern, Suche in Dateien und die drei
    /// Agenten-Schalter (`editor_svc::settings`). Fehlt in älteren Dateien (dann die Standards, Agenten-Schalter aus);
    /// Unlesbares ergibt je Wert den Standard.
    #[serde(deserialize_with = "crate::editor_svc::settings::EditorSettings::lenient")]
    pub editor: crate::editor_svc::settings::EditorSettings,
    /// Nach einem Update die Tour „Was ist neu“ von selbst starten (seit 3.0.0, #143; Standard aus: dann nur ein Hinweis
    /// in der Seitenleiste).
    #[serde(deserialize_with = "lenient_false")]
    pub tutorial_whats_new: bool,
    /// Reiter „Zeit“ (seit 3.2.0, #164): Klang/Toast je Art, Blinken, Lautstärke, Klingeldauer, Zeiterfassung an Tickets,
    /// Sekunden der Weltuhren (`time::settings`). Fehlt in älteren Dateien (dann die Standards); Unlesbares je Wert der Standard.
    #[serde(deserialize_with = "crate::time::settings::TimeSettings::lenient", skip_serializing_if = "crate::time::settings::TimeSettings::is_default")]
    pub time: crate::time::settings::TimeSettings,
}

/// Liste von Texten tolerant lesen: Einträge mit falschem Typ fallen weg, ein falscher Typ insgesamt ergibt eine leere Liste.
fn lenient_strings<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default())
}

/// Tolerant lesen: nur `false` schaltet aus, alles andere (auch ein falscher Typ) ist `true`.
fn lenient_true<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(v.as_bool() != Some(false))
}

/// Tolerant lesen: nur `true` schaltet ein, alles andere (auch ein falscher Typ) ist `false`.
fn lenient_false<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(v.as_bool() == Some(true))
}

/// `wsl_recent` tolerant lesen: je Distribution eine Liste von Pfaden; ein Eintrag mit falschem Typ (kein Text, keine Liste)
/// fällt allein weg, die übrigen bleiben.
fn lenient_wsl_recent<'de, D: serde::Deserializer<'de>>(d: D) -> Result<std::collections::BTreeMap<String, Vec<String>>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    let mut m: std::collections::BTreeMap<String, Vec<String>> = std::collections::BTreeMap::new();
    if let serde_json::Value::Object(o) = v {
        for (distro, paths) in o {
            let list: Vec<String> = paths.as_array().map(|a| a.iter().filter_map(|p| p.as_str().map(str::to_string)).collect()).unwrap_or_default();
            if !list.is_empty() {
                m.insert(distro, list);
            }
        }
    }
    crate::wsl::sanitize_recent(&mut m);
    Ok(m)
}

/// Was das Mausrad in einem Vollbildprogramm (alternativer Bildschirm) tut, das keine Mausereignisse angefordert hat.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AltScreenWheel {
    /// Pfeiltasten senden (Alternate Scroll Mode, DECSET 1007) – wie Alacritty, GNOME Terminal, kitty.
    #[default]
    Arrows,
    /// Nichts senden.
    Off,
}

impl AltScreenWheel {
    /// Tolerant lesen: nur `"off"` schaltet aus, alles andere (auch ein unbekannter Wert) ist der Standard.
    fn lenient<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        Ok(if v.as_str() == Some("off") { AltScreenWheel::Off } else { AltScreenWheel::Arrows })
    }
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
            reduce_transparency: false,
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
            mcp_session_env: false,
            mcp_env_allow: Vec::new(),
            agent_general: crate::access::AgentAccess::AS_BEFORE,
            agent_create_projects: false,
            agent_delete: false,
            todo_new_private: false,
            show_system_notes: true,
            mcp_prompt_lang: Lang::En,
            backup_interval_hours: 24,
            backup_keep: 2,
            backup_dir: String::new(),
            undo_steps: 25,
            update_auto_check: false,
            update_last_check: 0,
            update_skip_version: String::new(),
            llm_servers: Default::default(),
            agent_tools: Default::default(),
            last_chat_model: None,
            alt_screen_wheel: AltScreenWheel::Arrows,
            remember_sessions: true,
            shift_enter_newline_powershell: false,
            palette: Default::default(),
            osc52_copy: true,
            warn_multiline_paste: true,
            claude_defaults: Default::default(),
            last_claude_opts: None,
            claude_bypass_dropped: false,
            bundled_conpty: true,
            wsl_recent: Default::default(),
            inherit_env: false,
            program_notify: true,
            bell_toast: false,
            progress_indicator: true,
            cursor_blink: true,
            theme_checks_off: Vec::new(),
            chat_prompt_parts: Default::default(),
            chat_compact: Default::default(),
            editor: Default::default(),
            tutorial_whats_new: false,
            time: Default::default(),
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
    /// Fenster der Sitzung: 0 = Hauptfenster, sonst die Kennung eines weiteren Fensters (`SavedState::extra_windows`).
    /// Fehlt bei Sitzungen des Hauptfensters (ältere Versionen kennen das Feld nicht und zeigen alles im Hauptfenster).
    #[serde(skip_serializing_if = "is_zero")]
    pub window: u64,
    /// Art der Sitzung (seit 2.2.0): Terminal (Standard, fehlt in der Datei) oder Chat mit einem lokalen Modell.
    /// Unbekannte Werte (neuere Versionen) laden als Terminal.
    #[serde(skip_serializing_if = "SessionKind::is_terminal", deserialize_with = "SessionKind::lenient")]
    pub kind: SessionKind,
    /// Server und Modell eines Chats (nur bei `kind = chat`); der Verlauf steht in `chats/<id>.json`.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient_chat_meta")]
    pub chat: Option<ChatMeta>,
    /// Bereich minimiert (seit 2.2.0): die Sitzung läuft weiter, steht aber in keinem Layout, bis sie zurückgeholt wird.
    /// Fehlt in der Datei, wenn nicht minimiert.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub minimized: bool,
    /// **Deprecated, nur zur Kompatibilität behalten, entfällt in 3.0.0.** 2.2.0 merkte hier den alten Platz eines
    /// minimierten Bereichs. Seit 2.3.0 öffnet ein zurückgeholter Bereich allein: Der Wert wird unverändert durchgereicht
    /// (beliebiges JSON, nie ausgewertet, nie neu angelegt) und gelöscht, sobald die Sitzung minimiert oder wieder gezeigt wird.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slot: Option<serde_json::Value>,
    /// Startoptionen für Claude Code (seit 2.3.0): Modell, Effort, Permission-Mode, Agent. Fehlt in der Datei, wenn keine
    /// gesetzt sind; ungültige Werte werden beim Laden verworfen. Ältere Sitzungen haben sie ggf. noch in `claude_args`.
    #[serde(skip_serializing_if = "crate::claude_opts::ClaudeOpts::is_empty")]
    pub claude_opts: crate::claude_opts::ClaudeOpts,
    /// Agenten-Kette (seit 2.3.0, #152): Sitzungen, von denen diese Sitzung abstammt – ihr Ersteller über
    /// `create_session` samt dessen Kette, und jeder Agent, der über MCP in sie geschrieben hat, samt seiner Kette.
    /// Ein Agent in dieser Sitzung darf in keine davon schreiben. Aufsteigend, ohne Doppelte; fehlt in der Datei, wenn
    /// leer (vom Nutzer gestartet und nie von einem Agenten beschrieben). Ungültige Werte laden als leer.
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "lenient_chain")]
    pub agent_chain: Vec<u64>,
    /// Claude-Sitzung (seit 2.3.0): „Neu laden“/„Neu starten“ setzt die Konversation fort (`--resume`) statt eine neue
    /// zu beginnen. Standard an (so verhielt sich der Neustart bisher); fehlt in der Datei, solange es an ist.
    /// Ab 2.4.1 (#302) nur noch Spiegel von `claude_resume_mode` für ältere Versionen: `false` = „Aus“, sonst an.
    #[serde(skip_serializing_if = "is_true", deserialize_with = "lenient_bool_true")]
    pub claude_continue: bool,
    /// „Fortsetzen nach Neustart“ (seit 2.4.1, #302): `off` / `continue` / `pick`. Fehlt = aus `claude_continue`
    /// abgeleitet (`true` → `continue`, `false` → `off`); unbekannte Werte laden wie ein fehlendes Feld (und gehen beim
    /// Speichern verloren). Geschrieben wird
    /// es nur, wenn es nicht `continue` ist; `claude_continue` wird weiter mitgeschrieben (ältere Versionen).
    /// Lesen und Setzen nur über [`SessionMeta::resume_mode`] / [`SessionMeta::set_resume_mode`].
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "ResumeMode::lenient")]
    pub claude_resume_mode: Option<ResumeMode>,
    /// Eigene Umgebungsvariablen der Sitzung (seit 3.2.0, #384): Name, Wert (normal im Klartext, geheim nur als
    /// `value_protected`), `secret`, `mode`, `remove`, `wsl_path`, `by_agent` (`session_env::EnvVar`). Fehlt, wenn leer;
    /// unlesbare oder geschützte Einträge laden nicht.
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "crate::session_env::lenient_list")]
    pub env: Vec<crate::session_env::EnvVar>,
}

/// „Fortsetzen nach Neustart“ einer Claude-Sitzung (#302).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResumeMode {
    /// Jeder Start beginnt eine neue Konversation.
    Off,
    /// Erster Start neu, ab dem ersten Neu laden/Neustart `claude --continue` (sofern es im Ordner schon eine gibt).
    #[default]
    Continue,
    /// Bei jedem Neu laden/Neustart die Konversation aus der Liste des Arbeitsordners auswählen.
    Pick,
}

impl ResumeMode {
    /// Aus `claude_continue` (Dateien vor 2.4.1).
    pub fn from_bool(on: bool) -> Self {
        if on { ResumeMode::Continue } else { ResumeMode::Off }
    }

    /// Nachsichtig lesen: unbekannte Werte und falsche Typen zählen als fehlend (dann entscheidet `claude_continue`).
    pub fn lenient<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Self>, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        Ok(serde_json::from_value(v).ok())
    }

    /// Wert für Felder neben `claude_continue`: nur, was nicht der Standard ist.
    pub fn stored(self) -> Option<Self> {
        (self != ResumeMode::Continue).then_some(self)
    }
}

impl SessionMeta {
    /// Wirksames „Fortsetzen nach Neustart“ (#302).
    pub fn resume_mode(&self) -> ResumeMode {
        self.claude_resume_mode.unwrap_or(ResumeMode::from_bool(self.claude_continue))
    }

    /// Setzt den Modus samt `claude_continue` (für ältere Versionen: an bei `continue`/`pick`).
    pub fn set_resume_mode(&mut self, m: ResumeMode) {
        self.claude_resume_mode = m.stored();
        self.claude_continue = m != ResumeMode::Off;
    }
}

fn is_true(v: &bool) -> bool {
    *v
}

/// Nur ein echtes `false` schaltet aus; ein falscher Typ (z. B. aus einer fremden Version) lädt als an.
fn lenient_bool_true<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(v.as_bool().unwrap_or(true))
}

/// `agent_chain` nachsichtig lesen: eine Liste von Zahlen; Fremdes (kein Array, keine Zahlen) wird übergangen.
fn lenient_chain<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<u64>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    let mut out: Vec<u64> = v.as_array().map(|a| a.iter().filter_map(|x| x.as_u64()).collect()).unwrap_or_default();
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// Art einer Sitzung.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionKind {
    #[default]
    Terminal,
    Chat,
}

impl SessionKind {
    fn is_terminal(&self) -> bool {
        *self == SessionKind::Terminal
    }

    /// Tolerant lesen: alles außer `"chat"` (auch ein unbekannter Wert oder ein falscher Typ) ist ein Terminal.
    fn lenient<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        Ok(if v.as_str() == Some("chat") { SessionKind::Chat } else { SessionKind::Terminal })
    }
}

/// Modell eines Chats: Kennung des Servers (Einstellungen) und Modellname, fest ab der Anlage.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatMeta {
    pub server: String,
    pub model: String,
}

/// Ungültige Chat-Angaben verwerfen die Sitzung nicht (sie wird dann ohne Modell geladen).
fn lenient_chat_meta<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<ChatMeta>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value::<ChatMeta>(v).ok())
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

impl SessionMeta {
    /// Wirksame Claude-Optionen und die übrigen (bereinigten) freien `claude_args`. Sitzungen von vor 2.3.0 haben
    /// Modell/Permission-Mode noch in `claude_args`; sie zählen, solange das neue Feld leer ist.
    pub fn claude_opts_eff(&self) -> (crate::claude_opts::ClaudeOpts, String) {
        // Nie ein `--dangerously-*`-Flag aus alten Zeilen; ein für diese Sitzung gewähltes bypassPermissions bleibt.
        // Wertoptionen aus der alten Zeile, die die Optionen kennen, gelten weiter (sie liefen in dieser Sitzung schon).
        let c = crate::claude_opts::clean_legacy_report(&self.claude_args, true);
        let (mut legacy, rest) = crate::claude_opts::ClaudeOpts::split_legacy(&c.line);
        legacy.extra = legacy.extra.or(&c.migrated);
        (self.claude_opts.or(&legacy), rest)
    }

    /// Alte `claude_args` einmalig umstellen (Review zu #245): Wertoptionen, die die Claude-Optionen kennen, wandern
    /// gültig geprüft in `claude_opts` (ein dort schon gesetzter Wert gilt weiter), Weggelassenes verschwindet aus der
    /// Zeile. Rückgabe: die weggelassenen Flags (leer = nichts zu melden); danach liefert ein zweiter Aufruf nichts mehr.
    pub fn migrate_legacy_args(&mut self) -> Vec<String> {
        let c = crate::claude_opts::clean_legacy_report(&self.claude_args, true);
        if c.dropped.is_empty() && c.migrated.is_empty() {
            return Vec::new();
        }
        self.claude_args = c.line;
        self.claude_opts.extra = self.claude_opts.extra.or(&c.migrated);
        c.dropped
    }
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
            window: 0,
            kind: SessionKind::Terminal,
            chat: None,
            minimized: false,
            slot: None,
            claude_continue: true,
            claude_resume_mode: None,
            claude_opts: Default::default(),
            agent_chain: Vec::new(),
            env: Vec::new(),
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
    #[serde(deserialize_with = "crate::layout::lenient")]
    pub layout: Option<Node>,
    pub focused: u64,
    /// Seitenleiste als Ordnerbaum statt als Liste anzeigen.
    pub tree_view: bool,
    /// Ordner dieses Projekts (Pfade mit `/`, auch leere).
    pub folders: Vec<Folder>,
    /// Zugriff der Agenten (#386). Fehlt (ältere `state.json`) = wie global, für alle; neue Projekte beginnen gesperrt.
    pub agent_access: crate::access::AgentAccess,
}

impl Default for Project {
    fn default() -> Self {
        Self { id: 0, name: String::new(), color: [0x61, 0xaf, 0xef], folder: String::new(), profile: 0, layout: None, focused: 0, tree_view: false, folders: Vec::new(), agent_access: crate::access::AgentAccess::AS_BEFORE }
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

    /// Löscht den Ordner mit Unterordnern (und ihren Vorgaben) aus der Liste (Papierkorb am Ordner, #276).
    pub fn remove_folder(&mut self, path: &str) {
        if path.is_empty() {
            return;
        }
        self.folders.retain(|f| !folder_within(&f.path, path));
    }

    /// Wohin die direkten Unterordner von `path` beim Löschen (Inhalt bleibt) rücken: (alter Pfad, neuer Pfad). Gibt es
    /// im übergeordneten Ordner schon einen Ordner dieses Namens, bekommt der hochgerückte einen freien Namen
    /// („c (2)“) – sonst flössen beide zusammen und die Vorgaben des hochgerückten gingen verloren (#276).
    pub fn lift_plan(&self, path: &str) -> Vec<(String, String)> {
        let parent = folder_parent(path);
        let mut taken: std::collections::HashSet<String> = self.folders.iter().filter(|f| !folder_within(&f.path, path)).map(|f| f.path.clone()).collect();
        let mut kids: Vec<&Folder> = self.folders.iter().filter(|f| folder_parent(&f.path) == path && f.path != path).collect();
        kids.sort_by(|a, b| a.path.cmp(&b.path));
        let join = |leaf: &str| if parent.is_empty() { leaf.to_string() } else { format!("{parent}/{leaf}") };
        let mut plan = Vec::new();
        for k in kids {
            let leaf = folder_leaf(&k.path);
            let mut new = join(leaf);
            let mut n = 2;
            while taken.contains(&new) {
                new = join(&format!("{leaf} ({n})"));
                n += 1;
            }
            taken.insert(new.clone());
            plan.push((k.path.clone(), new));
        }
        plan
    }

    /// Löscht nur den Ordner selbst; Unterordner rücken eine Ebene hoch ([`Project::lift_plan`], [`lift_mapped`]). Seine
    /// Vorgaben für neue Sitzungen (Arbeitsordner, Profil) gehen an die direkten Unterordner, die keine eigenen haben –
    /// so starten neue Sitzungen darin wie vorher (#276). Gibt den Plan zurück (für die Sitzungen).
    pub fn delete_folder_lift(&mut self, path: &str) -> Vec<(String, String)> {
        if path.is_empty() {
            return Vec::new();
        }
        let plan = self.lift_plan(path);
        if let Some(gone) = self.folder(path).cloned() {
            for f in self.folders.iter_mut().filter(|f| folder_parent(&f.path) == path) {
                if f.cwd.is_empty() {
                    f.cwd = gone.cwd.clone();
                }
                if f.profile == 0 {
                    f.profile = gone.profile;
                }
            }
        }
        self.folders.retain(|f| f.path != path);
        for f in &mut self.folders {
            f.path = lift_mapped(&f.path, path, &plan);
        }
        let mut seen = std::collections::HashSet::new();
        self.folders.retain(|f| !f.path.is_empty() && seen.insert(f.path.clone()));
        plan
    }
}

/// Pfad nach dem Löschen von `deleted` (Inhalt bleibt) mit dem Plan aus [`Project::lift_plan`]; Pfade ohne Eintrag im
/// Plan wie [`lift_path`].
pub fn lift_mapped(p: &str, deleted: &str, plan: &[(String, String)]) -> String {
    for (old, new) in plan {
        if folder_within(p, old) {
            return format!("{new}{}", &p[old.len()..]);
        }
    }
    lift_path(p, deleted)
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SavedState {
    pub sessions: Vec<SessionMeta>,
    /// Alter Einzel-Layout-Stand (vor Projekten); wird beim Laden ins erste Projekt übernommen.
    #[serde(deserialize_with = "crate::layout::lenient")]
    pub layout: Option<Node>,
    pub focused: u64,
    pub next_id: u64,
    pub projects: Vec<Project>,
    pub active_project: u64,
    /// In eigenen Fenstern abgedockte Sitzungen.
    pub detached: Vec<u64>,
    pub composer_draft: String,
    /// Weitere Fenster (Strg+Klick auf einen Reiter); fehlt, solange es keine gibt.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extra_windows: Vec<SavedWindow>,
    /// Nächste Kennung eines Seitenleisten-Projekts (#386): Kennungen werden nie wiederverwendet, sonst erbten neue
    /// Projekte Karten und Verknüpfungen gelöschter. Fehlt (ältere Dateien) = größte Kennung + 1.
    #[serde(skip_serializing_if = "is_zero")]
    pub next_project_id: u64,
}

impl SavedState {
    /// Nächste freie Projekt-Kennung: nie unter einer schon vergebenen.
    pub fn project_id_floor(&self) -> u64 {
        self.next_project_id.max(self.projects.iter().map(|p| p.id).max().unwrap_or(0) + 1)
    }
}

/// Ein weiteres Fenster (#110): Reiter, Projekt und Layout des Terminal Hubs, Lage auf dem Bildschirm.
/// Seine Sitzungen stehen in `sessions` mit `window = id`.
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SavedWindow {
    /// Kennung (ab 1; 0 ist das Hauptfenster).
    pub id: u64,
    /// Angezeigter Reiter: `"terminals"` oder `"tasks"`.
    pub tab: String,
    /// Aktives Projekt des Terminal Hubs in diesem Fenster.
    pub project: u64,
    #[serde(deserialize_with = "crate::layout::lenient")]
    pub layout: Option<Node>,
    pub focused: u64,
    /// Layouts der übrigen Projekte in diesem Fenster.
    pub views: Vec<SavedProjectView>,
    /// Position (links oben) und Innengröße in Punkten.
    pub pos: Option<[f32; 2]>,
    pub size: Option<[f32; 2]>,
    pub maximized: bool,
}

/// Layout eines Projekts in einem weiteren Fenster.
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SavedProjectView {
    pub project: u64,
    #[serde(deserialize_with = "crate::layout::lenient")]
    pub layout: Option<Node>,
    pub focused: u64,
}

/// Datenordner dieses Prozesses (einmal aufgelöst, siehe `paths`): `--data-dir` > `PANELDECK_DATA` >
/// `paneldeck-data` neben der .exe > %APPDATA%/Paneldeck, mit `--profile` jeweils `profiles\<name>` darunter.
pub fn data_dir() -> PathBuf {
    crate::paths::get().path.clone()
}

/// Sicherung der Einstellungen nach `--reset-settings` (für den Hinweis beim Start).
pub static SETTINGS_RESET: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// `--reset-settings`: `settings.json` in `settings.json.bak-<JJJJMMTT-HHMMSS>` umbenennen (bei Namensgleichheit mit
/// Zähler). Gibt die Sicherung zurück; `Ok(None)`, wenn es keine Einstellungsdatei gab.
pub fn reset_settings(dir: &std::path::Path) -> std::io::Result<Option<PathBuf>> {
    let src = dir.join("settings.json");
    if !src.exists() {
        return Ok(None);
    }
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let mut target = dir.join(format!("settings.json.bak-{stamp}"));
    let mut n = 2;
    while target.exists() {
        target = dir.join(format!("settings.json.bak-{stamp}-{n}"));
        n += 1;
    }
    std::fs::rename(&src, &target)?;
    Ok(Some(target))
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
    if let Err(e) = write_private(&tmp, data) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Neue Datei schreiben; Linux/macOS (#149 Review): nur für den Benutzer lesbar (0600) – Datenordner-Dateien wie
/// `ipc.json` (Token), `settings.json`, `state.json`, Chats. Eine vorhandene Datei wird vorher entfernt, damit die
/// Rechte sicher neu gesetzt sind.
pub fn write_private(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let _ = std::fs::remove_file(path);
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
        f.write_all(data)
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, data)
    }
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
        // bypassPermissions ist nie ein gespeicherter Standard und nie die „letzte Wahl“ (nur ausdrücklich im Dialog).
        if self.claude_defaults.permission_mode == crate::claude_opts::BYPASS {
            self.claude_bypass_dropped = true;
        }
        self.claude_defaults = self.claude_defaults.sanitized(false);
        self.last_claude_opts = self.last_claude_opts.as_ref().map(|o| o.sanitized(false));
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
        crate::llm::config::sanitize(&mut self.llm_servers);
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
    fn lifting_into_an_existing_name_keeps_both_folders() {
        // #276: a/b löschen, obwohl a/c schon existiert: a/b/c wird „a/c (2)“ (mit seinen Vorgaben und Unterordnern).
        let mut p = Project::default();
        for f in ["a/c", "a/b/c/d", "a/b/e"] {
            p.ensure_folder(f);
        }
        p.folders.iter_mut().find(|f| f.path == "a/b/c").unwrap().cwd = "C:/bc".into();
        let plan = p.lift_plan("a/b");
        assert_eq!(plan, vec![("a/b/c".to_string(), "a/c (2)".to_string()), ("a/b/e".to_string(), "a/e".to_string())]);
        assert_eq!(lift_mapped("a/b/c/d", "a/b", &plan), "a/c (2)/d");
        assert_eq!(lift_mapped("a/b", "a/b", &plan), "a");
        assert_eq!(lift_mapped("a/b/x", "a/b", &plan), "a/x", "ohne Ordnereintrag wie lift_path");
        assert_eq!(p.delete_folder_lift("a/b"), plan);
        let mut v: Vec<_> = p.folders.iter().map(|f| f.path.as_str()).collect();
        v.sort();
        assert_eq!(v, ["a", "a/c", "a/c (2)", "a/c (2)/d", "a/e"]);
        assert_eq!(p.folder("a/c (2)").map(|f| f.cwd.as_str()), Some("C:/bc"));
        assert_eq!(p.folder("a/c").map(|f| f.cwd.as_str()), Some(""));
        // oberste Ebene
        let mut p = Project::default();
        for f in ["x", "t/x"] {
            p.ensure_folder(f);
        }
        assert_eq!(p.lift_plan("t"), vec![("t/x".to_string(), "x (2)".to_string())]);
    }
    #[test]
    fn deleting_a_folder_keeps_the_defaults_consistent() {
        // Löschen mit Inhalt (#276): Unterordner und ihre Vorgaben verschwinden, nichts bleibt verwaist.
        let mut p = Project::default();
        for f in ["a/b/c", "a/bx", "z"] {
            p.ensure_folder(f);
        }
        p.folders.iter_mut().find(|f| f.path == "a/b").unwrap().cwd = "C:/b".into();
        p.remove_folder("a/b");
        let v: Vec<_> = p.folders.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(v, ["a", "a/bx", "z"], "nur gleicher Anfang (a/bx) bleibt");
        p.remove_folder("");
        assert_eq!(p.folders.len(), 3, "leerer Pfad löscht nichts");
        // Löschen, Inhalt bleibt: die Vorgaben gehen an direkte Unterordner ohne eigene.
        let mut p = Project::default();
        for f in ["a/b/c", "a/b/d", "a/b/d/e"] {
            p.ensure_folder(f);
        }
        let set = |p: &mut Project, path: &str, cwd: &str, profile: u64| {
            let f = p.folders.iter_mut().find(|f| f.path == path).unwrap();
            f.cwd = cwd.into();
            f.profile = profile;
        };
        set(&mut p, "a/b", "C:/b", 7);
        set(&mut p, "a/b/d", "C:/d", 0);
        let before_c = p.inherited("a/b/c", |f| Some(f.cwd.clone()).filter(|c| !c.is_empty()));
        p.delete_folder_lift("a/b");
        assert_eq!(p.folder("a/c").map(|f| (f.cwd.as_str(), f.profile)), Some(("C:/b", 7)));
        assert_eq!(p.folder("a/d").map(|f| (f.cwd.as_str(), f.profile)), Some(("C:/d", 7)), "eigener Arbeitsordner bleibt");
        assert_eq!(p.folder("a/d/e").map(|f| (f.cwd.as_str(), f.profile)), Some(("", 0)), "tiefer liegende erben weiter");
        assert_eq!(p.inherited("a/c", |f| Some(f.cwd.clone()).filter(|c| !c.is_empty())), before_c);
        assert!(p.folder("a/b").is_none());
        // Gespeichert und wieder geladen: kein Eintrag des gelöschten Ordners mehr (auch alte Dateien laden weiter).
        let j = serde_json::to_string(&p).unwrap();
        assert!(!j.contains("\"a/b\""), "{j}");
        let old: Project = serde_json::from_str(r#"{"id":1,"name":"P","folders":[{"path":"a","cwd":"C:/x"}]}"#).unwrap();
        assert_eq!(old.folders[0].cwd, "C:/x");
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

    #[test]
    fn lang_override_is_not_persisted() {
        // reine Funktion (der globale Zustand bleibt unberührt, Tests laufen parallel)
        assert_eq!(persisted_language(Lang::De, Some((Lang::De, Lang::En))), Lang::En);
        assert_eq!(persisted_language(Lang::En, Some((Lang::De, Lang::En))), Lang::En);
        assert_eq!(persisted_language(Lang::De, Some((Lang::En, Lang::En))), Lang::De);
        assert_eq!(persisted_language(Lang::De, None), Lang::De);
    }

    #[test]
    fn lang_override_never_reaches_settings_json() {
        // threadlokal: betrifft nur diesen Test-Thread
        let mut s = Settings { language: Lang::En, ..Settings::default() };
        set_lang_override(Lang::De, Lang::En);
        s.language = Lang::De; // so setzt App::new die Startsprache
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["language"], "En");
        assert_eq!(persisted_lang(s.language), Lang::En);
        // eigene Wahl in den Einstellungen beendet die Überschreibung
        clear_lang_override();
        assert_eq!(serde_json::to_value(&s).unwrap()["language"], "De");
        assert_eq!(persisted_lang(Lang::De), Lang::De);
    }

    #[test]
    fn reset_settings_keeps_backup() {
        let dir = std::env::temp_dir().join(format!("pd-reset-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(reset_settings(&dir).unwrap().is_none());
        std::fs::write(dir.join("settings.json"), "{\"language\":\"De\"}").unwrap();
        std::fs::write(dir.join("state.json"), "{}").unwrap();
        assert_eq!(lang_from_disk(&dir), Some(Lang::De));
        let b1 = reset_settings(&dir).unwrap().unwrap();
        assert!(b1.file_name().unwrap().to_string_lossy().starts_with("settings.json.bak-"));
        assert!(!dir.join("settings.json").exists() && dir.join("state.json").exists());
        std::fs::write(dir.join("settings.json"), "{}").unwrap();
        let b2 = reset_settings(&dir).unwrap().unwrap();
        assert_ne!(b1, b2);
        assert_eq!(std::fs::read_to_string(&b1).unwrap(), "{\"language\":\"De\"}");
        assert_eq!(lang_from_disk(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn update_settings_default_and_roundtrip() {
        // settings.json von 2.0.0 (ohne Update-Felder): automatische Prüfung bleibt aus
        let old = r#"{"language":"De","theme":"One Dark","font_size":15.0,"backup_keep":3,"undo_steps":10}"#;
        let s: Settings = serde_json::from_str(old).unwrap();
        assert!(!s.update_auto_check);
        assert_eq!(s.update_last_check, 0);
        assert!(s.update_skip_version.is_empty());
        assert_eq!(s.font_size, 15.0);
        assert_eq!(s.backup_keep, 3);
        assert!(!Settings::default().update_auto_check, "Standard: aus");
        // Hin und zurück
        let mut s = Settings::default();
        s.update_auto_check = true;
        s.update_last_check = 1_800_000_000;
        s.update_skip_version = "3.0.0".into();
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert!(back.update_auto_check);
        assert_eq!(back.update_last_check, 1_800_000_000);
        assert_eq!(back.update_skip_version, "3.0.0");
    }

    #[test]
    fn llm_settings_back_compat_and_roundtrip() {
        // settings.json von 2.1.0 (ohne LLM-Felder) lädt; die neuen Felder fehlen dann auch beim Schreiben.
        let old = r#"{"language":"En","mcp_level":"Read","update_auto_check":true}"#;
        let s: Settings = serde_json::from_str(old).unwrap();
        assert!(s.llm_servers.is_empty() && s.last_chat_model.is_none());
        let j = serde_json::to_value(&s).unwrap();
        assert!(j.get("llm_servers").is_none() && j.get("last_chat_model").is_none());
        // Ein unlesbarer Server (unbekannter Typ) und ein kaputtes last_chat_model machen die Datei nicht unlesbar.
        let new = r#"{"language":"De","llm_servers":[{"id":"a","name":"O","kind":"ollama","url":"http://localhost:11434","models":[{"name":"m","level":"Control","max_steps":0}]},{"id":"b","kind":"gemini"}],"last_chat_model":7}"#;
        let mut s: Settings = serde_json::from_str(new).unwrap();
        s.sanitize();
        assert_eq!(s.language, Lang::De);
        assert_eq!(s.llm_servers.len(), 1);
        assert_eq!(s.llm_servers.unreadable, vec![serde_json::json!({"id":"b","kind":"gemini"})], "bleibt erhalten");
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j["llm_servers"][1], serde_json::json!({"id":"b","kind":"gemini"}), "und wird zurückgeschrieben");
        assert_eq!(s.llm_servers[0].models[0].max_steps, 1, "auf 1..=100 begrenzt");
        assert!(s.last_chat_model.is_none());
        s.last_chat_model = Some(ChatMeta { server: "a".into(), model: "m".into() });
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.llm_servers, s.llm_servers);
        assert_eq!(back.last_chat_model, s.last_chat_model);
    }

    #[test]
    fn remember_sessions_back_compat_and_roundtrip() {
        // settings.json von 2.2.0 (ohne das Feld) lädt mit „an“ (bisheriges Verhalten), die übrigen Werte bleiben.
        let old = r#"{"language":"De","confirm_close":false,"alt_screen_wheel":"off"}"#;
        let s: Settings = serde_json::from_str(old).unwrap();
        assert!(s.remember_sessions);
        assert!(!s.confirm_close);
        assert_eq!(s.alt_screen_wheel, AltScreenWheel::Off);
        assert!(Settings::default().remember_sessions);
        // Aus bleibt aus.
        let mut s = s;
        s.remember_sessions = false;
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j["remember_sessions"], false);
        let back: Settings = serde_json::from_value(j).unwrap();
        assert!(!back.remember_sessions);
    }

    #[test]
    fn inherit_env_back_compat_and_roundtrip() {
        // settings.json von 2.2.0 (ohne das Feld) lädt mit „aus“ (Umgebung aus der Registry wie bisher).
        let s: Settings = serde_json::from_str(r#"{"language":"En","shell_integration":false}"#).unwrap();
        assert!(!s.inherit_env);
        assert!(!s.shell_integration);
        assert!(!Settings::default().inherit_env);
        let mut s = s;
        s.inherit_env = true;
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j["inherit_env"], true);
        assert!(serde_json::from_value::<Settings>(j).unwrap().inherit_env);
    }

    #[test]
    fn bundled_conpty_back_compat_and_roundtrip() {
        // settings.json von 2.2.0 (ohne das Feld) lädt mit „an“, die übrigen Werte bleiben.
        let old = r#"{"language":"De","confirm_close":false,"remember_sessions":false}"#;
        let s: Settings = serde_json::from_str(old).unwrap();
        assert!(s.bundled_conpty);
        assert!(!s.confirm_close && !s.remember_sessions);
        assert!(Settings::default().bundled_conpty);
        // Aus bleibt aus.
        let mut s = s;
        s.bundled_conpty = false;
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j["bundled_conpty"], false);
        let back: Settings = serde_json::from_value(j).unwrap();
        assert!(!back.bundled_conpty);
        // Falscher Typ: an, die übrigen Einstellungen bleiben lesbar.
        for bad in [r#""no""#, "0", "null", "{}"] {
            let s: Settings = serde_json::from_str(&format!(r#"{{"language":"De","bundled_conpty":{bad}}}"#)).unwrap();
            assert!(s.bundled_conpty, "{bad}");
            assert_eq!(s.language, Lang::De);
        }
    }

    #[test]
    fn osc52_copy_back_compat_and_roundtrip() {
        // settings.json von 2.2.0 (ohne das Feld) lädt mit „an“, die übrigen Werte bleiben.
        let old = r#"{"language":"De","confirm_close":false}"#;
        let s: Settings = serde_json::from_str(old).unwrap();
        assert!(s.osc52_copy);
        assert!(!s.confirm_close);
        assert!(Settings::default().osc52_copy);
        let mut s = s;
        s.osc52_copy = false;
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j["osc52_copy"], false);
        let back: Settings = serde_json::from_value(j).unwrap();
        assert!(!back.osc52_copy);
    }

    #[test]
    fn warn_multiline_paste_back_compat_and_roundtrip() {
        // settings.json von 2.2.0 (ohne das Feld) lädt mit „an“, die übrigen Werte bleiben.
        let old = r#"{"language":"En","osc52_copy":false}"#;
        let s: Settings = serde_json::from_str(old).unwrap();
        assert!(s.warn_multiline_paste);
        assert!(!s.osc52_copy);
        assert!(Settings::default().warn_multiline_paste);
        let mut s = s;
        s.warn_multiline_paste = false;
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j["warn_multiline_paste"], false);
        let back: Settings = serde_json::from_value(j).unwrap();
        assert!(!back.warn_multiline_paste);
    }

    #[test]
    fn vt_render_settings_back_compat_and_roundtrip() {
        // settings.json von 2.2.0 (ohne die Felder): alles an; ein falscher Typ ergibt ebenfalls an, nur `false` schaltet aus.
        let s: Settings = serde_json::from_str(r#"{"language":"En"}"#).unwrap();
        assert!(s.program_notify && !s.bell_toast && s.progress_indicator && s.cursor_blink);
        let s: Settings = serde_json::from_str(r#"{"program_notify":"no","bell_toast":true,"progress_indicator":1,"cursor_blink":false}"#).unwrap();
        assert!(s.program_notify && s.bell_toast && s.progress_indicator && !s.cursor_blink);
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!((j["bell_toast"].as_bool(), j["cursor_blink"].as_bool(), j["program_notify"].as_bool()), (Some(true), Some(false), Some(true)));
        let back: Settings = serde_json::from_value(j).unwrap();
        assert!(back.bell_toast && !back.cursor_blink);
    }

    #[test]
    fn wsl_recent_back_compat_and_roundtrip() {
        // settings.json von 2.2.0 (ohne das Feld) lädt mit leerer Liste und schreibt das Feld nicht.
        let old = r#"{"language":"De","remember_sessions":false}"#;
        let s: Settings = serde_json::from_str(old).unwrap();
        assert!(s.wsl_recent.is_empty());
        assert!(!s.remember_sessions);
        assert!(serde_json::to_value(&s).unwrap().get("wsl_recent").is_none());
        // Kaputter Wert: Standard statt Ladefehler; ungültige Einträge fallen weg.
        let s: Settings = serde_json::from_str(r#"{"wsl_recent": 5, "confirm_close": false}"#).unwrap();
        assert!(s.wsl_recent.is_empty() && !s.confirm_close);
        let s: Settings = serde_json::from_str(r#"{"wsl_recent": {"Ubuntu": ["/home/u", "rel", "/x\n"], "-x": ["/a"]}}"#).unwrap();
        assert_eq!(s.wsl_recent.len(), 1);
        assert_eq!(s.wsl_recent["Ubuntu"], vec!["/home/u".to_string()]);
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.wsl_recent, s.wsl_recent);
    }

    #[test]
    fn new_2_3_settings_with_a_wrong_type_do_not_reset_the_file() {
        // #221: ein falscher Typ in einem der Felder aus 2.3.0 ergibt dessen Standard, die übrigen Einstellungen bleiben.
        let j = r#"{"language":"De","confirm_close":false,"font_size":17.0,
            "remember_sessions":"no","osc52_copy":0,"warn_multiline_paste":null,"shift_enter_newline_powershell":"yes",
            "inherit_env":1,"bell_toast":{},"program_notify":[],"progress_indicator":"x","cursor_blink":5,"bundled_conpty":"off",
            "claude_defaults":7,"last_claude_opts":"x","palette":3,"wsl_recent":[1]}"#;
        let s: Settings = serde_json::from_str(j).expect("darf nicht scheitern");
        assert_eq!(s.language, Lang::De);
        assert!(!s.confirm_close && s.font_size == 17.0, "übrige Einstellungen bleiben");
        let d = Settings::default();
        assert_eq!((s.remember_sessions, s.osc52_copy, s.warn_multiline_paste), (d.remember_sessions, d.osc52_copy, d.warn_multiline_paste));
        assert_eq!((s.shift_enter_newline_powershell, s.inherit_env, s.bell_toast), (false, false, false));
        assert!(s.program_notify && s.progress_indicator && s.cursor_blink && s.bundled_conpty);
        // Theme-Prüfung (#309): falsche Typen werden toleriert.
        let s: Settings = serde_json::from_str(r#"{"theme_checks_off":["contrast",3,"harmony"]}"#).unwrap();
        assert_eq!(s.theme_checks_off, ["contrast", "harmony"]);
        let s: Settings = serde_json::from_str(r#"{"theme_checks_off":"x"}"#).unwrap();
        assert!(s.theme_checks_off.is_empty());
        // Transparenz reduzieren (#387): fehlt in alten Dateien = aus; falscher Typ = aus; wird geschrieben und gelesen.
        assert!(!Settings::default().reduce_transparency && !s.reduce_transparency);
        let s: Settings = serde_json::from_str(r#"{"reduce_transparency":"ja"}"#).unwrap();
        assert!(!s.reduce_transparency);
        let s: Settings = serde_json::from_str(r#"{"reduce_transparency":true}"#).unwrap();
        assert!(s.reduce_transparency);
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert!(back.reduce_transparency);
        // Prompt-Teile der Chats (#341): fehlt = alles an und wird nicht geschrieben; falscher Typ = alles an.
        use crate::llm::prompt_parts::PromptPart;
        assert!(s.chat_prompt_parts.is_default());
        assert!(serde_json::to_value(&s).unwrap().get("chat_prompt_parts").is_none());
        let s: Settings = serde_json::from_str(r#"{"chat_prompt_parts":{"safety":false,"examples":"off"}}"#).unwrap();
        assert!(!s.chat_prompt_parts.get(PromptPart::Safety) && s.chat_prompt_parts.get(PromptPart::Examples));
        assert_eq!(serde_json::to_value(&s).unwrap()["chat_prompt_parts"]["safety"], serde_json::json!(false));
        assert!(serde_json::from_str::<Settings>(r#"{"chat_prompt_parts":7}"#).unwrap().chat_prompt_parts.is_default());
        // Kompaktieren (#359): fehlt in älteren Dateien (Standard: aus) und wird dann auch nicht geschrieben.
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert!(!s.chat_compact.auto && s.chat_compact.is_default());
        assert!(serde_json::to_value(&s).unwrap().get("chat_compact").is_none());
        let s: Settings = serde_json::from_str(r#"{"chat_compact":{"auto":true,"threshold_percent":70}}"#).unwrap();
        assert!(s.chat_compact.auto && s.chat_compact.threshold_percent == 70 && s.chat_compact.window_min == 4096);
        assert_eq!(serde_json::to_value(&s).unwrap()["chat_compact"], serde_json::json!({"auto": true, "threshold_percent": 70, "window_percent": 15, "window_min": 4096}));
        assert!(serde_json::from_str::<Settings>(r#"{"chat_compact":"on"}"#).unwrap().chat_compact.is_default());
        // Editor (#54): fehlt = Standards (Agenten-Schalter aus), Unlesbares je Wert, Rundweg über settings.json.
        assert_eq!(s.editor, crate::editor_svc::settings::EditorSettings::default());
        let e: Settings = serde_json::from_str(r#"{"editor":{"agent_open":true,"large_file_mb":"x","search_exclude":["dist/"]},"theme":"Dracula"}"#).unwrap();
        assert!(e.editor.agent_open && !e.editor.agent_edit && e.editor.large_file_mb == 20 && e.editor.search_exclude == ["dist/"] && e.theme == "Dracula");
        let back: Settings = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(back.editor, e.editor);
        let e: Settings = serde_json::from_str(r#"{"editor":"kaputt","theme":"Dracula"}"#).unwrap();
        assert!(!e.editor.agent_open && e.theme == "Dracula", "ein kaputtes editor-Feld kostet nicht die übrigen Einstellungen");
        assert!(s.claude_defaults.is_empty() && s.last_claude_opts.as_ref().is_none_or(|o| o.is_empty()));
        assert!(s.wsl_recent.is_empty());
        // echte Werte gelten weiter
        let s: Settings = serde_json::from_str(r#"{"remember_sessions":false,"osc52_copy":false,"warn_multiline_paste":false,"shift_enter_newline_powershell":true,"inherit_env":true,"bell_toast":true}"#).unwrap();
        assert!(!s.remember_sessions && !s.osc52_copy && !s.warn_multiline_paste && s.shift_enter_newline_powershell && s.inherit_env && s.bell_toast);
    }

    #[test]
    fn wsl_recent_drops_only_the_bad_entry() {
        // #221: eine Distribution mit falschem Typ (oder ein Pfad, der kein Text ist) fällt allein weg.
        let s: Settings = serde_json::from_str(r#"{"wsl_recent": {"Ubuntu": ["/home/u", 5, "/srv"], "Debian": 3, "Arch": "/x", "Alpine": ["/a"]}}"#).unwrap();
        assert_eq!(s.wsl_recent.len(), 2, "{:?}", s.wsl_recent);
        assert_eq!(s.wsl_recent["Ubuntu"], vec!["/home/u".to_string(), "/srv".to_string()]);
        assert_eq!(s.wsl_recent["Alpine"], vec!["/a".to_string()]);
    }

    #[test]
    fn shift_enter_setting_back_compat_and_roundtrip() {
        // settings.json ohne das Feld (vor 2.3.0): aus.
        let s: Settings = serde_json::from_str(r#"{"language":"En","scrollback_lines":5000}"#).unwrap();
        assert!(!s.shift_enter_newline_powershell);
        let mut s = s;
        s.shift_enter_newline_powershell = true;
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j["shift_enter_newline_powershell"], true);
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert!(back.shift_enter_newline_powershell);
    }

    #[test]
    fn alt_screen_wheel_back_compat_and_roundtrip() {
        // settings.json von 2.1.0 (ohne das Feld) lädt mit dem Standard „Pfeiltasten“.
        let old = r#"{"language":"En","scrollback_lines":5000}"#;
        let s: Settings = serde_json::from_str(old).unwrap();
        assert_eq!(s.alt_screen_wheel, AltScreenWheel::Arrows);
        assert_eq!(s.scrollback_lines, 5000);
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j["alt_screen_wheel"], "arrows");
        // Gespeicherter Wert bleibt erhalten.
        let mut s = s;
        s.alt_screen_wheel = AltScreenWheel::Off;
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.alt_screen_wheel, AltScreenWheel::Off);
        // Unbekannter Wert oder falscher Typ: Standard, die übrigen Einstellungen bleiben lesbar.
        for bad in [r#""report""#, "3", "null", r#"{"x":1}"#] {
            let s: Settings = serde_json::from_str(&format!(r#"{{"language":"De","alt_screen_wheel":{bad}}}"#)).unwrap();
            assert_eq!(s.alt_screen_wheel, AltScreenWheel::Arrows, "{bad}");
            assert_eq!(s.language, Lang::De);
        }
    }
}

#[cfg(test)]
mod kind_tests {
    use super::*;

    #[test]
    fn old_state_without_kind_loads_as_terminal() {
        // state.json von 2.1.0: Sitzungen ohne `kind`/`chat`
        let dir = std::env::temp_dir().join(format!("pd-kind-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("state.json");
        std::fs::write(&p, r#"{"sessions":[{"id":3,"name":"pwsh","shell_id":"pwsh","project":1}],"next_id":4}"#).unwrap();
        let (st, note) = load_saved(&p);
        assert!(note.is_none());
        assert_eq!(st.sessions.len(), 1);
        assert_eq!(st.sessions[0].kind, SessionKind::Terminal);
        assert!(st.sessions[0].chat.is_none());
        // Terminals schreiben die neuen Felder nicht (ältere Versionen sehen dieselbe Datei wie bisher)
        let s = serde_json::to_string(&st.sessions[0]).unwrap();
        assert!(!s.contains("\"kind\"") && !s.contains("\"chat\""), "{s}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn claude_opts_back_compat_and_roundtrip() {
        use crate::claude_opts::ClaudeOpts;
        // state.json/settings.json von 2.2.0 (ohne die Felder) laden; geschrieben werden sie dann auch nicht.
        let m: SessionMeta = serde_json::from_str(r#"{"id":3,"name":"c","claude":true,"claude_args":"--model opus"}"#).unwrap();
        assert!(m.claude_opts.is_empty());
        assert!(!serde_json::to_string(&m).unwrap().contains("claude_opts"));
        let s: Settings = serde_json::from_str(r#"{"language":"En","claude_mode":true}"#).unwrap();
        assert!(s.claude_defaults.is_empty() && s.last_claude_opts.is_none());
        let j = serde_json::to_value(&s).unwrap();
        assert!(j.get("claude_defaults").is_none() && j.get("last_claude_opts").is_none());
        // Kaputte Werte machen die Dateien nicht unlesbar; ungültige Felder werden verworfen.
        let m: SessionMeta = serde_json::from_str(r#"{"id":4,"claude_opts":{"model":"opus;calc","effort":"high","permission_mode":"bypassPermissions"}}"#).unwrap();
        assert_eq!(m.claude_opts, ClaudeOpts { effort: "high".into(), permission_mode: "bypassPermissions".into(), ..Default::default() });
        let s: Settings = serde_json::from_str(r#"{"claude_defaults":7,"last_claude_opts":"x"}"#).unwrap();
        assert!(s.claude_defaults.is_empty());
        // bypassPermissions als Standard (ältere Datei / Hand-Bearbeitung): wird beim Laden verworfen, mit Hinweis.
        let mut s: Settings = serde_json::from_str(r#"{"claude_defaults":{"model":"opus","permission_mode":"bypassPermissions"},"last_claude_opts":{"permission_mode":"bypassPermissions","effort":"low"}}"#).unwrap();
        s.sanitize();
        assert!(s.claude_bypass_dropped);
        assert_eq!(s.claude_defaults, ClaudeOpts { model: "opus".into(), ..Default::default() });
        assert_eq!(s.last_claude_opts, Some(ClaudeOpts { effort: "low".into(), ..Default::default() }));
        let mut ok = Settings::default();
        ok.sanitize();
        assert!(!ok.claude_bypass_dropped);
        // Auch ungesäubert im Speicher (Einstellungsseite während der Eingabe) wird nichts Ungültiges geschrieben.
        let mut s = Settings::default();
        s.claude_defaults = ClaudeOpts { model: "op us".into(), permission_mode: "bypassPermissions".into(), agent: "rev".into(), ..Default::default() };
        s.last_claude_opts = Some(ClaudeOpts { permission_mode: "bypassPermissions".into(), ..Default::default() });
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j["claude_defaults"], serde_json::json!({"agent": "rev"}));
        assert_eq!(j["last_claude_opts"], serde_json::json!({}));
        assert!(j.get("claude_bypass_dropped").is_none());
        // Hin und zurück
        let mut m = SessionMeta::default();
        m.claude_opts = ClaudeOpts { model: "sonnet".into(), effort: "max".into(), permission_mode: "plan".into(), agent: "rev".into(), ..Default::default() };
        let back: SessionMeta = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back.claude_opts, m.claude_opts);
        let mut s = Settings::default();
        s.claude_defaults = ClaudeOpts { model: "opus".into(), ..Default::default() };
        s.last_claude_opts = Some(ClaudeOpts { effort: "low".into(), ..Default::default() });
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!((back.claude_defaults, back.last_claude_opts), (s.claude_defaults, s.last_claude_opts));
        // Sitzungen von vor 2.3.0: Modell/Permission-Mode aus `claude_args` zählen, solange das neue Feld leer ist.
        let mut m: SessionMeta = serde_json::from_str(r#"{"id":5,"claude":true,"claude_args":"--model opus --permission-mode plan --verbose --keep $(x)"}"#).unwrap();
        let (o, rest) = m.claude_opts_eff();
        assert_eq!((o.model.as_str(), o.permission_mode.as_str(), rest.as_str()), ("opus", "plan", "--keep"));
        assert_eq!(o.extra.flags, ["verbose"], "Schalter aus alten claude_args erscheinen als Schalter (#182)");
        m.claude_opts.model = "haiku".into();
        assert_eq!(m.claude_opts_eff().0.model, "haiku", "das neue Feld hat Vorrang");
        m.claude_args = "--dangerously-skip-permissions --keep".into();
        assert_eq!(m.claude_opts_eff().1, "--keep", "nie --dangerously-*");
    }

    #[test]
    fn claude_extra_opts_back_compat_and_roundtrip() {
        use crate::claude_opts::{ClaudeOpts, ExtraOpts};
        // Dateien von vor #182 (nur die vier Optionen) laden unverändert, ohne weitere Felder.
        let m: SessionMeta = serde_json::from_str(r#"{"id":3,"claude":true,"claude_opts":{"model":"opus"}}"#).unwrap();
        assert!(m.claude_opts.extra.is_empty());
        let j = serde_json::to_value(&m).unwrap();
        assert_eq!(j["claude_opts"], serde_json::json!({"model": "opus"}), "keine neuen Felder, solange nichts gesetzt ist");
        let s: Settings = serde_json::from_str(r#"{"claude_defaults":{"effort":"high"}}"#).unwrap();
        assert!(s.claude_defaults.extra.is_empty());
        // Neue Felder: Schalter je Sitzung in state.json, Standards in settings.json (auch die Rechte erweiternden).
        let extra = ExtraOpts {
            flags: vec!["chrome".into(), "verbose".into()],
            add_dirs: if cfg!(windows) { vec!["C:\\work".into()] } else { vec!["/work".into()] },
            allowed_tools: "Read".into(),
            disallowed_tools: "WebFetch".into(),
            mcp_configs: vec![if cfg!(windows) { "C:\\m.json".into() } else { "/m.json".into() }],
            plugin_dirs: if cfg!(windows) { vec!["C:\\plug".into()] } else { vec!["/plug".into()] },
            strict_mcp_config: true,
            system_prompt: "Line 1\nLine 2".into(),
            append_system_prompt: "Be brief.".into(),
            fallback_model: "sonnet".into(),
            max_budget_usd: "5".into(),
            autocompact: "auto".into(),
        };
        let mut m = SessionMeta::default();
        m.claude_opts = ClaudeOpts { extra: extra.clone(), ..Default::default() };
        let j = serde_json::to_value(&m).unwrap();
        assert_eq!(j["claude_opts"]["flags"], serde_json::json!(["chrome", "verbose"]));
        let back: SessionMeta = serde_json::from_value(j).unwrap();
        assert_eq!(back.claude_opts.extra, extra);
        let mut s = Settings::default();
        s.claude_defaults = ClaudeOpts { extra: extra.clone(), ..Default::default() };
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.claude_defaults.extra, extra);
        // Kaputte Werte: verworfen, kein Ladefehler.
        let m: SessionMeta = serde_json::from_str(r#"{"id":4,"claude_opts":{"flags":"chrome","add_dirs":[1,"\\\\srv\\x"],"max_budget_usd":"-1"}}"#).unwrap();
        assert!(m.claude_opts.extra.is_empty());
    }

    #[test]
    fn agent_chain_survives_a_restart_and_old_files_load_without_it() {
        let dir = std::env::temp_dir().join(format!("pd-chain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("state.json");
        // 2.2.0 und älter: kein Feld → leere Kette (vom Nutzer gestartet)
        std::fs::write(&p, r#"{"sessions":[{"id":3,"name":"pwsh","shell_id":"pwsh","project":1}],"next_id":4}"#).unwrap();
        let (st, _) = load_saved(&p);
        assert!(st.sessions[0].agent_chain.is_empty());
        // Leere Kette wird nicht geschrieben (ältere Versionen sehen dieselbe Datei)
        assert!(!serde_json::to_string(&st.sessions[0]).unwrap().contains("agent_chain"));
        // Neustart: die Kette bleibt an der Sitzung (Ids werden nie neu vergeben)
        let saved = SavedState { sessions: vec![SessionMeta { id: 12, agent_chain: vec![5, 10], ..SessionMeta::default() }], next_id: 13, ..SavedState::default() };
        save_json(&p, &saved);
        let (st, note) = load_saved(&p);
        assert!(note.is_none());
        assert_eq!(st.sessions[0].agent_chain, [5, 10]);
        // Fremde Werte: nachsichtig (sortiert, ohne Doppelte, nur Zahlen; kein Array → leer)
        let m: SessionMeta = serde_json::from_str(r#"{"id":1,"agent_chain":[9,"x",3,9,-1]}"#).unwrap();
        assert_eq!(m.agent_chain, [3, 9]);
        let m: SessionMeta = serde_json::from_str(r#"{"id":1,"agent_chain":"5"}"#).unwrap();
        assert!(m.agent_chain.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn chat_kind_roundtrip_and_unknown_kind_is_terminal() {
        let m = SessionMeta { id: 7, kind: SessionKind::Chat, chat: Some(ChatMeta { server: "echo".into(), model: "m1".into() }), ..SessionMeta::default() };
        let s = serde_json::to_string(&m).unwrap();
        assert!(s.contains(r#""kind":"chat""#), "{s}");
        let back: SessionMeta = serde_json::from_str(&s).unwrap();
        assert_eq!(back.kind, SessionKind::Chat);
        assert_eq!(back.chat.as_ref().map(|c| c.model.as_str()), Some("m1"));
        // Unbekannte Art (neuere Version) und falsche Typen: Sitzung bleibt erhalten, als Terminal bzw. ohne Modell
        let back: SessionMeta = serde_json::from_str(r#"{"id":8,"kind":"notebook","chat":42}"#).unwrap();
        assert_eq!(back.kind, SessionKind::Terminal);
        assert!(back.chat.is_none());
        let back: SessionMeta = serde_json::from_str(r#"{"id":9,"kind":7}"#).unwrap();
        assert_eq!(back.kind, SessionKind::Terminal);
    }

    #[test]
    fn minimized_sessions_round_trip_and_old_files_load() {
        // 2.1.0: kein `minimized` → nicht minimiert; nicht minimierte Sitzungen schreiben das Feld auch nicht
        let old: SessionMeta = serde_json::from_str(r#"{"id":3,"name":"pwsh"}"#).unwrap();
        assert!(!old.minimized);
        let s = serde_json::to_string(&old).unwrap();
        assert!(!s.contains("minimized") && !s.contains("slot"), "{s}");
        // minimiert (2.3.0): Rundreise, ohne `slot` (wird nie neu angelegt)
        let m = SessionMeta { id: 4, minimized: true, ..SessionMeta::default() };
        let s = serde_json::to_string(&m).unwrap();
        assert!(s.contains("\"minimized\":true") && !s.contains("slot"), "{s}");
        assert!(serde_json::from_str::<SessionMeta>(&s).unwrap().minimized);
        // 2.2.0 mit `slot` (gültig oder kaputt): lädt, der Wert wird unverändert durchgereicht (deprecated bis 3.0.0)
        for (old, slot) in [
            (r#"{"id":5,"minimized":true,"slot":{"sibling":[1,2],"dir":"Vertical","before":false,"ratio":0.3}}"#, r#"{"sibling":[1,2],"dir":"Vertical","before":false,"ratio":0.3}"#),
            (r#"{"id":5,"minimized":true,"slot":{"dir":"Up"}}"#, r#"{"dir":"Up"}"#),
            (r#"{"id":5,"minimized":true,"slot":17}"#, "17"),
        ] {
            let back: SessionMeta = serde_json::from_str(old).unwrap_or_else(|e| panic!("{old}: {e}"));
            assert!(back.id == 5 && back.minimized, "{old}");
            let want: serde_json::Value = serde_json::from_str(slot).unwrap();
            assert_eq!(back.slot.as_ref(), Some(&want), "{old}");
            let again: SessionMeta = serde_json::from_str(&serde_json::to_string(&back).unwrap()).unwrap();
            assert_eq!(again.slot, Some(want), "{old}: Rundreise");
        }
        // `null` → kein Wert, wird nicht geschrieben
        let back: SessionMeta = serde_json::from_str(r#"{"id":5,"minimized":true,"slot":null}"#).unwrap();
        assert!(back.minimized && back.slot.is_none());
        assert!(!serde_json::to_string(&back).unwrap().contains("slot"));
    }

    #[test]
    fn state_file_of_2_2_0_with_slots_loads() {
        let dir = std::env::temp_dir().join(format!("pd-slot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("state.json");
        std::fs::write(
            &p,
            r#"{"sessions":[{"id":1},{"id":2,"minimized":true,"slot":{"sibling":[1],"dir":"Horizontal","before":false,"ratio":0.5}},
                            {"id":3,"minimized":true,"slot":"kaputt"}],
                "next_id":4,"active_project":1,"projects":[{"id":1,"name":"A","layout":{"Leaf":1},"focused":1}]}"#,
        )
        .unwrap();
        let (st, note) = load_saved(&p);
        assert!(note.is_none(), "{note:?}");
        assert_eq!(st.sessions.len(), 3);
        assert!(!st.sessions[0].minimized && st.sessions[1].minimized && st.sessions[2].minimized);
        assert!(st.sessions[0].slot.is_none() && st.sessions[1].slot.is_some());
        assert_eq!(st.sessions[2].slot, Some(serde_json::json!("kaputt")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn claude_continue_defaults_on_and_round_trips() {
        // 2.2.0: kein `claude_continue` → an (Neustart setzt die Konversation fort wie bisher); an wird nicht geschrieben
        let old: SessionMeta = serde_json::from_str(r#"{"id":3,"name":"Claude","claude":true,"claude_session":"x"}"#).unwrap();
        assert!(old.claude_continue);
        assert!(SessionMeta::default().claude_continue);
        let s = serde_json::to_string(&old).unwrap();
        assert!(!s.contains("claude_continue"), "{s}");
        // aus: Rundreise
        let m = SessionMeta { id: 4, claude: true, claude_continue: false, ..SessionMeta::default() };
        let s = serde_json::to_string(&m).unwrap();
        assert!(s.contains(r#""claude_continue":false"#), "{s}");
        let back: SessionMeta = serde_json::from_str(&s).unwrap();
        assert!(!back.claude_continue);
        // falscher Typ: Sitzung bleibt, Option an
        let back: SessionMeta = serde_json::from_str(r#"{"id":5,"claude_continue":"no"}"#).unwrap();
        assert!(back.claude_continue);
        assert_eq!(back.id, 5);
    }

    /// #302: „Fortsetzen nach Neustart“ mit drei Werten; alte Dateien (nur `claude_continue`) werden übernommen,
    /// `claude_continue` wird für ältere Versionen weiter geschrieben.
    #[test]
    fn claude_resume_mode_migrates_from_the_old_bool_and_round_trips() {
        // alt: an/fehlt → fortsetzen, aus → aus
        let old: SessionMeta = serde_json::from_str(r#"{"id":1,"claude":true}"#).unwrap();
        assert_eq!(old.resume_mode(), ResumeMode::Continue);
        let old: SessionMeta = serde_json::from_str(r#"{"id":1,"claude":true,"claude_continue":false}"#).unwrap();
        assert_eq!(old.resume_mode(), ResumeMode::Off);
        // neu: jeder Modus übersteht die Rundreise; ältere Versionen lesen `claude_continue`
        for (m, cont, field) in [
            (ResumeMode::Off, false, Some(r#""claude_resume_mode":"off""#)),
            (ResumeMode::Continue, true, None),
            (ResumeMode::Pick, true, Some(r#""claude_resume_mode":"pick""#)),
        ] {
            let mut meta = SessionMeta { id: 2, claude: true, ..SessionMeta::default() };
            meta.set_resume_mode(m);
            assert_eq!(meta.claude_continue, cont, "{m:?}");
            let s = serde_json::to_string(&meta).unwrap();
            match field {
                Some(f) => assert!(s.contains(f), "{s}"),
                None => assert!(!s.contains("claude_resume_mode") && !s.contains("claude_continue"), "{s}"),
            }
            assert_eq!(s.contains(r#""claude_continue":false"#), !cont, "{s}");
            let back: SessionMeta = serde_json::from_str(&s).unwrap();
            assert_eq!(back.resume_mode(), m);
        }
        // Das neue Feld gewinnt; unbekannte Werte/Typen laden wie fehlend (dann zählt `claude_continue`)
        let m: SessionMeta = serde_json::from_str(r#"{"id":3,"claude_continue":true,"claude_resume_mode":"pick"}"#).unwrap();
        assert_eq!(m.resume_mode(), ResumeMode::Pick);
        let m: SessionMeta = serde_json::from_str(r#"{"id":3,"claude_continue":false,"claude_resume_mode":"später"}"#).unwrap();
        assert_eq!((m.id, m.resume_mode()), (3, ResumeMode::Off));
        let m: SessionMeta = serde_json::from_str(r#"{"id":3,"claude_resume_mode":7}"#).unwrap();
        assert_eq!(m.resume_mode(), ResumeMode::Continue);
    }

    #[test]
    fn broken_layouts_do_not_discard_the_state_file() {
        use crate::layout::Node;
        let dir = std::env::temp_dir().join(format!("pd-layout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("state.json");
        // Projekt 1: Anteil null (so schreibt serde_json ein NaN), Projekt 2: unlesbarer Knoten, Fenster: falscher Typ
        std::fs::write(
            &p,
            r#"{"sessions":[{"id":1},{"id":2,"minimized":true}],"next_id":3,"active_project":1,
                "projects":[{"id":1,"name":"A","layout":{"Split":{"dir":"Horizontal","ratio":null,"a":{"Leaf":1},"b":{"Leaf":1}}}},
                            {"id":2,"name":"B","layout":{"Split":{"dir":"Sideways"}}}],
                "extra_windows":[{"id":1,"layout":17,"views":[{"project":2,"layout":[1,2]}]}]}"#,
        )
        .unwrap();
        let (st, note) = load_saved(&p);
        assert!(note.is_none(), "{note:?}");
        assert_eq!(st.sessions.len(), 2);
        assert!(st.sessions[1].minimized);
        assert_eq!(st.projects.len(), 2, "Projekte bleiben erhalten");
        match &st.projects[0].layout {
            Some(Node::Split { ratio, .. }) => assert_eq!(*ratio, 0.5),
            other => panic!("{other:?}"),
        }
        assert!(st.projects[1].layout.is_none(), "kaputtes Layout → kein Layout");
        assert!(st.extra_windows[0].layout.is_none() && st.extra_windows[0].views[0].layout.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
