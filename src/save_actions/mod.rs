//! Aktionen beim Speichern (#383, 3.2.0): Regeln je Datei, Ordner (rekursiv) oder Muster, die beim Speichern im
//! Editor-Reiter laufen – eingebaute Aktionen, die den Text vor dem Schreiben ändern (Leerraum am Zeilenende, letzter
//! Zeilenumbruch, Zeilenenden, Kodierung, Tabs↔Leerzeichen, Formatieren), danach Befehle, Kopieren an einen anderen
//! lokalen Ort und Paneldeck-Aktionen (Befehl an eine Sitzung, Prozess neu starten, Benachrichtigung).
//!
//! Ohne egui. Aufteilung:
//! - hier: Datenformat `save-actions.json` (öffentlich ab 3.2.0, Version 1; tolerant, unbekannte Felder bleiben),
//! - `matching`: welche Regeln für eine Datei und einen Auslöser gelten (Reihenfolge, Stopp-Schalter, Muster),
//! - `vars`: Variablen `${file}` … mit Quoting je Shell, Umgebungsvariablen `PANELDECK_FILE` …,
//! - `queue`: Warteschlange (je Datei höchstens ein Lauf plus genau ein wartender, Parallelität),
//! - `builtin`: die Text-Aktionen (vor dem Schreiben),
//! - `copy`: Kopieren (atomar, nur lokal, nie in den Regelordner),
//! - `exchange`: Export/Import ohne geheime Werte.
//!
//! Sicherheit: Regeln legt nur der Nutzer an (Agenten lesen sie höchstens über `list_save_actions`, ohne Befehle und
//! Werte). Paneldeck selbst greift dabei nie aufs Netz zu – Uploads nur über eigene Befehle des Nutzers.

pub mod builtin;
pub mod copy;
pub mod exchange;
pub mod life;
pub mod matching;
pub mod queue;
pub mod vars;

use crate::procs::EnvVar;
use crate::state::Lang;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Datei im Datenordner.
pub const FILE: &str = "save-actions.json";
/// Version des Datenformats.
pub const FORMAT_VERSION: u32 = 1;
/// Zeitlimit eines Befehls: Standard, kleinster und größter Wert (Sekunden).
pub const TIMEOUT_DEFAULT: u32 = 60;
pub const TIMEOUT_MIN: u32 = 1;
pub const TIMEOUT_MAX: u32 = 3600;
/// Parallele Läufe (Speichern aller Dateien): Standard und Grenzen.
pub const PARALLEL_DEFAULT: u8 = 2;
pub const PARALLEL_MIN: u8 = 1;
pub const PARALLEL_MAX: u8 = 8;
/// So viele Läufe behält der Ausgabebereich (nur im Speicher).
pub const HISTORY_RUNS: usize = 100;
/// So viele Zeilen Ausgabe je Lauf.
pub const OUTPUT_LINES: usize = 2000;
/// Ordner der Protokolldateien (rotierend wie `process-logs/`).
pub const LOG_DIR: &str = "save-action-logs";
/// Höchstzahl Regeln (Schutz vor kaputten Dateien).
pub const MAX_RULES: usize = 500;
/// Höchstzahl Aktionen je Regel.
pub const MAX_ACTIONS: usize = 50;

fn is_false(v: &bool) -> bool {
    !*v
}
fn is_true(v: &bool) -> bool {
    *v
}
fn yes() -> bool {
    true
}
fn timeout_default() -> u32 {
    TIMEOUT_DEFAULT
}
fn is_timeout_default(v: &u32) -> bool {
    *v == TIMEOUT_DEFAULT
}

/// Wofür eine Regel gilt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// Genau eine Datei (`path` = die Datei).
    File,
    /// Alle Dateien in einem Ordner und seinen Unterordnern (`path` = der Ordner).
    #[default]
    Folder,
    /// Dateien im Ordner `path`, die zu `patterns` passen (Syntax wie `.gitignore`, relativ zum Ordner).
    Pattern,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::File => "file",
            Scope::Folder => "folder",
            Scope::Pattern => "pattern",
        }
    }

    pub fn label(self, lang: Lang) -> &'static str {
        match self {
            Scope::File => lang.t("Datei", "File"),
            Scope::Folder => lang.t("Ordner", "Folder"),
            Scope::Pattern => lang.t("Muster", "Pattern"),
        }
    }
}

/// Eingebaute Aktion, die den Text vor dem Schreiben ändert (schnell, im UI-Thread).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Builtin {
    /// Formatieren (JSON/JSONC/XML über `editor::format`, siehe `builtin::format_text`).
    Format,
    /// Leerzeichen/Tabs am Zeilenende entfernen.
    TrimWhitespace,
    /// Am Ende genau einen Zeilenumbruch sicherstellen (fehlt er, kommt er dazu).
    FinalNewline,
    /// Zeilenenden beim Schreiben (`lf`/`crlf`).
    LineEndings(crate::editor::LineEnding),
    /// Kodierung beim Schreiben.
    Encoding(crate::editor::TextEncoding),
    /// Einrückung mit Tabs in Leerzeichen umwandeln (Breite 0 = Tabbreite des Dokuments).
    TabsToSpaces(u8),
    /// Einrückung mit Leerzeichen in Tabs umwandeln (Breite 0 = Tabbreite des Dokuments).
    SpacesToTabs(u8),
}

impl Builtin {
    pub fn kind(&self) -> &'static str {
        match self {
            Builtin::Format => "format",
            Builtin::TrimWhitespace => "trim_whitespace",
            Builtin::FinalNewline => "final_newline",
            Builtin::LineEndings(_) => "line_endings",
            Builtin::Encoding(_) => "encoding",
            Builtin::TabsToSpaces(_) => "tabs_to_spaces",
            Builtin::SpacesToTabs(_) => "spaces_to_tabs",
        }
    }

    pub fn label(&self, lang: Lang) -> String {
        match self {
            Builtin::Format => lang.t("Formatieren (JSON/XML)", "Format (JSON/XML)").into(),
            Builtin::TrimWhitespace => lang.t("Leerraum am Zeilenende entfernen", "Trim trailing whitespace").into(),
            Builtin::FinalNewline => lang.t("Zeilenumbruch am Ende", "Final newline").into(),
            Builtin::LineEndings(e) => format!("{} {}", lang.t("Zeilenenden:", "Line endings:"), if *e == crate::editor::LineEnding::CrLf { "CRLF" } else { "LF" }),
            Builtin::Encoding(e) => format!("{} {}", lang.t("Kodierung:", "Encoding:"), e.id()),
            Builtin::TabsToSpaces(w) => width_label(lang.t("Tabs → Leerzeichen", "Tabs → spaces"), *w, lang),
            Builtin::SpacesToTabs(w) => width_label(lang.t("Leerzeichen → Tabs", "Spaces → tabs"), *w, lang),
        }
    }
}

fn width_label(base: &str, w: u8, lang: Lang) -> String {
    if w == 0 { format!("{base} ({})", lang.t("Tabbreite der Datei", "file's tab width")) } else { format!("{base} ({w})") }
}

/// Was eine Aktion tut.
#[derive(Clone, Debug, PartialEq)]
pub enum ActionKind {
    Builtin(Builtin),
    /// Befehl über die Shell der Regel (verborgen bzw. sichtbar im Terminal); Variablen `${file}` usw.
    Command { command: String },
    /// Datei an einen anderen lokalen Ort kopieren: `target` = Zielordner; `keep_structure` = Pfad relativ zum
    /// Regelordner beibehalten (sonst nur der Dateiname).
    Copy { target: String, keep_structure: bool },
    /// Text (Befehl) an eine Sitzung senden – nur, wenn sie an ihrem Prompt wartet. `name` = gemerkter Anzeigename.
    SendToSession { session: u64, name: String, text: String },
    /// Prozess der Prozessliste neu starten (Kennung) bzw. eine ganze Gruppe (`group` nicht leer). `name` = gemerkter
    /// Anzeigename.
    RestartProcess { process: u64, group: String, name: String },
    /// Benachrichtigung zeigen (Variablen erlaubt).
    Notify { text: String },
    /// Aktion einer neueren Version: bleibt beim Speichern erhalten, wird hier übersprungen.
    Unknown(Value),
}

impl ActionKind {
    /// Kennung für Datei und MCP (`kind`).
    pub fn kind(&self) -> String {
        match self {
            ActionKind::Builtin(b) => b.kind().into(),
            ActionKind::Command { .. } => "command".into(),
            ActionKind::Copy { .. } => "copy".into(),
            ActionKind::SendToSession { .. } => "send_to_session".into(),
            ActionKind::RestartProcess { .. } => "restart_process".into(),
            ActionKind::Notify { .. } => "notify".into(),
            ActionKind::Unknown(v) => v.get("kind").and_then(|k| k.as_str()).unwrap_or("unknown").to_string(),
        }
    }

    /// Ändert den Text vor dem Schreiben?
    pub fn modifies(&self) -> bool {
        matches!(self, ActionKind::Builtin(_))
    }

    pub fn label(&self, lang: Lang) -> String {
        match self {
            ActionKind::Builtin(b) => b.label(lang),
            ActionKind::Command { .. } => lang.t("Befehl ausführen", "Run a command").into(),
            ActionKind::Copy { .. } => lang.t("Datei kopieren", "Copy the file").into(),
            ActionKind::SendToSession { .. } => lang.t("An Sitzung senden", "Send to a session").into(),
            ActionKind::RestartProcess { .. } => lang.t("Prozess neu starten", "Restart a process").into(),
            ActionKind::Notify { .. } => lang.t("Benachrichtigung zeigen", "Show a notification").into(),
            ActionKind::Unknown(_) => format!("{} ({})", lang.t("Unbekannte Aktion", "Unknown action"), self.kind()),
        }
    }
}

/// Eine Aktion einer Regel.
#[derive(Clone, Debug, PartialEq)]
pub struct Action {
    pub kind: ActionKind,
    /// Bei einem Fehler trotzdem mit den übrigen Aktionen der Regel weitermachen.
    pub continue_on_error: bool,
}

impl Action {
    pub fn new(kind: ActionKind) -> Action {
        Action { kind, continue_on_error: false }
    }

    fn to_value(&self) -> Value {
        let mut m = match &self.kind {
            ActionKind::Unknown(v) => return v.clone(),
            ActionKind::Builtin(b) => {
                let mut m = Map::new();
                match b {
                    Builtin::LineEndings(e) => {
                        m.insert("eol".into(), json!(if *e == crate::editor::LineEnding::CrLf { "crlf" } else { "lf" }));
                    }
                    Builtin::Encoding(e) => {
                        m.insert("encoding".into(), json!(e.id()));
                    }
                    Builtin::TabsToSpaces(w) | Builtin::SpacesToTabs(w) if *w > 0 => {
                        m.insert("width".into(), json!(w));
                    }
                    _ => {}
                }
                m
            }
            ActionKind::Command { command } => obj(json!({"command": command})),
            ActionKind::Copy { target, keep_structure } => obj(json!({"target": target, "keep_structure": keep_structure})),
            ActionKind::SendToSession { session, name, text } => obj(json!({"session": session, "name": name, "text": text})),
            ActionKind::RestartProcess { process, group, name } if group.is_empty() => obj(json!({"process": process, "name": name})),
            ActionKind::RestartProcess { group, name, .. } => obj(json!({"group": group, "name": name})),
            ActionKind::Notify { text } => obj(json!({"text": text})),
        };
        m.insert("kind".into(), json!(self.kind.kind()));
        if self.continue_on_error {
            m.insert("continue_on_error".into(), json!(true));
        }
        Value::Object(m)
    }

    /// Aus JSON lesen; eine unbekannte Art bleibt roh erhalten. `None` nur ohne `kind`.
    fn from_value(v: &Value) -> Option<Action> {
        let o = v.as_object()?;
        let kind = o.get("kind")?.as_str()?;
        let s = |k: &str| o.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let u = |k: &str| o.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
        let width = || u("width").min(16) as u8;
        let k = match kind {
            "format" => ActionKind::Builtin(Builtin::Format),
            "trim_whitespace" => ActionKind::Builtin(Builtin::TrimWhitespace),
            "final_newline" => ActionKind::Builtin(Builtin::FinalNewline),
            "line_endings" => match crate::editor::LineEnding::from_id(&s("eol")) {
                Some(e) => ActionKind::Builtin(Builtin::LineEndings(e)),
                None => ActionKind::Unknown(v.clone()),
            },
            "encoding" => match crate::editor::TextEncoding::from_id(&s("encoding")) {
                Some(e) => ActionKind::Builtin(Builtin::Encoding(e)),
                None => ActionKind::Unknown(v.clone()),
            },
            "tabs_to_spaces" => ActionKind::Builtin(Builtin::TabsToSpaces(width())),
            "spaces_to_tabs" => ActionKind::Builtin(Builtin::SpacesToTabs(width())),
            "command" => ActionKind::Command { command: s("command") },
            "copy" => ActionKind::Copy { target: s("target"), keep_structure: o.get("keep_structure").and_then(|x| x.as_bool()).unwrap_or(true) },
            "send_to_session" => ActionKind::SendToSession { session: u("session"), name: s("name"), text: s("text") },
            "restart_process" => ActionKind::RestartProcess { process: u("process"), group: s("group"), name: s("name") },
            "notify" => ActionKind::Notify { text: s("text") },
            _ => ActionKind::Unknown(v.clone()),
        };
        Some(Action { kind: k, continue_on_error: o.get("continue_on_error").and_then(|x| x.as_bool()).unwrap_or(false) })
    }
}

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

impl Serialize for Action {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.to_value().serialize(s)
    }
}

impl<'de> Deserialize<'de> for Action {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        Action::from_value(&v).ok_or_else(|| serde::de::Error::custom("action without kind"))
    }
}

/// Eine Regel (ein Eintrag in `save-actions.json`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    /// Eindeutige Kennung (> 0), bleibt beim Umbenennen und Sortieren gleich.
    pub id: u64,
    #[serde(default)]
    pub name: String,
    /// Regel an/aus.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    #[serde(default)]
    pub scope: Scope,
    /// Datei (`file`) bzw. Regelordner (`folder`, `pattern`), absolut.
    #[serde(default)]
    pub path: String,
    /// Muster wie in `.gitignore` relativ zum Regelordner (nur `pattern`), z. B. `*.json`, `src/**/*.ts`, `!dist/**`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub patterns: Vec<String>,
    #[serde(default)]
    pub actions: Vec<Action>,
    /// Auch beim automatischen Speichern ausführen (Standard aus).
    #[serde(default, skip_serializing_if = "is_false")]
    pub on_autosave: bool,
    /// Auch beim Speichern durch einen Agenten ausführen (Standard aus).
    #[serde(default, skip_serializing_if = "is_false")]
    pub on_agent_save: bool,
    /// „Keine weiteren Regeln danach“.
    #[serde(default, skip_serializing_if = "is_false")]
    pub stop_after: bool,
    /// „Regeln übergeordneter Ordner nicht anwenden“.
    #[serde(default, skip_serializing_if = "is_false")]
    pub no_parent_rules: bool,
    /// Kennung einer erkannten Shell (wie in der Prozessliste); leer = PowerShell.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub shell: String,
    /// Eigene Umgebungsvariablen, Werte nur als DPAPI-Blob.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<EnvVar>,
    /// Arbeitsordner (leer = Regelordner, bei Datei-Regeln der Ordner der Datei).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cwd: String,
    /// Zeitlimit je Befehl in Sekunden (1–3600, Standard 60).
    #[serde(default = "timeout_default", skip_serializing_if = "is_timeout_default")]
    pub timeout_secs: u32,
    /// Befehle sichtbar in einem Terminal in der Seitenleiste ausführen (z. B. für Passwortabfragen).
    #[serde(default, skip_serializing_if = "is_false")]
    pub terminal: bool,
    /// Auch bei Erfolg benachrichtigen.
    #[serde(default, skip_serializing_if = "is_false")]
    pub notify_success: bool,
    /// Ausgabe zusätzlich in eine rotierende Protokolldatei schreiben (`save-action-logs/`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub log_file: bool,
    /// Unbekannte Felder (aus neueren Versionen) bleiben beim Speichern erhalten.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Rule {
    pub fn new(id: u64, scope: Scope, path: &str) -> Rule {
        Rule {
            id,
            name: String::new(),
            enabled: true,
            scope,
            path: path.to_string(),
            patterns: Vec::new(),
            actions: Vec::new(),
            on_autosave: false,
            on_agent_save: false,
            stop_after: false,
            no_parent_rules: false,
            shell: String::new(),
            env: Vec::new(),
            cwd: String::new(),
            timeout_secs: TIMEOUT_DEFAULT,
            terminal: false,
            notify_success: false,
            log_file: false,
            extra: BTreeMap::new(),
        }
    }

    /// Ordner, auf den sich Muster, `${relativeFile}` und der Arbeitsordner beziehen: bei Datei-Regeln der Ordner der
    /// Datei, sonst `path`.
    pub fn folder(&self) -> PathBuf {
        let p = PathBuf::from(self.path.trim());
        match self.scope {
            Scope::File => p.parent().map(Path::to_path_buf).unwrap_or(p),
            _ => p,
        }
    }

    /// Anzeigename (Name, sonst der Pfad).
    pub fn display_name(&self) -> String {
        let n = self.name.trim();
        if !n.is_empty() {
            return n.to_string();
        }
        let base = Path::new(self.path.trim()).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| self.path.trim().to_string());
        match self.scope {
            Scope::Pattern if !self.patterns.is_empty() => format!("{} ({base})", self.patterns.join(" ")),
            _ => base,
        }
    }

    /// Zeitlimit innerhalb der Grenzen.
    pub fn timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.timeout_secs.clamp(TIMEOUT_MIN, TIMEOUT_MAX) as u64)
    }

    /// Hat die Regel Befehle?
    pub fn has_commands(&self) -> bool {
        self.actions.iter().any(|a| matches!(a.kind, ActionKind::Command { .. }))
    }

    pub fn env_names(&self) -> Vec<String> {
        self.env.iter().map(|e| e.name.clone()).collect()
    }
}

/// Inhalt von `save-actions.json`.
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// Hauptschalter (Standard an – ohne Regeln passiert ohnehin nichts).
    pub enabled: bool,
    /// Parallele Läufe (Speichern aller Dateien), 1–8.
    pub parallel: u8,
    pub rules: Vec<Rule>,
    /// Unbekannte Felder der obersten Ebene.
    pub extra: Map<String, Value>,
}

impl Default for Config {
    fn default() -> Self {
        Config { enabled: true, parallel: PARALLEL_DEFAULT, rules: Vec::new(), extra: Map::new() }
    }
}

impl Config {
    pub fn next_id(&self) -> u64 {
        self.rules.iter().map(|r| r.id).max().unwrap_or(0) + 1
    }

    pub fn rule(&self, id: u64) -> Option<&Rule> {
        self.rules.iter().find(|r| r.id == id)
    }

    pub fn parallel(&self) -> usize {
        self.parallel.clamp(PARALLEL_MIN, PARALLEL_MAX) as usize
    }

    /// JSON der Datei.
    pub fn to_json(&self) -> Value {
        let mut m = self.extra.clone();
        m.insert("version".into(), json!(FORMAT_VERSION));
        m.insert("enabled".into(), json!(self.enabled));
        m.insert("parallel".into(), json!(self.parallel()));
        m.insert("rules".into(), serde_json::to_value(&self.rules).unwrap_or(Value::Array(Vec::new())));
        Value::Object(m)
    }
}

/// Zerlegter Inhalt.
#[derive(Debug, Default)]
pub struct Parsed {
    pub config: Config,
    pub bad: bool,
    pub version: u64,
}

/// Inhalt zerlegen (tolerant: ein ungültiger Eintrag wird übersprungen, doppelte/fehlende Kennungen neu vergeben).
pub fn parse(text: &str) -> Parsed {
    let Ok(v) = serde_json::from_str::<Value>(crate::state::strip_bom(text)) else { return Parsed { bad: true, version: 1, ..Default::default() } };
    let Some(o) = v.as_object() else { return Parsed { bad: true, version: 1, ..Default::default() } };
    let version = match o.get("version") {
        None => 1,
        Some(x) => x.as_u64().unwrap_or(u64::MAX),
    };
    let known = ["version", "enabled", "parallel", "rules"];
    let extra: Map<String, Value> = o.iter().filter(|(k, _)| !known.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect();
    let mut bad = false;
    let enabled = match o.get("enabled") {
        None => true,
        Some(Value::Bool(b)) => *b,
        Some(_) => {
            bad = true;
            true
        }
    };
    let parallel = o.get("parallel").and_then(|x| x.as_u64()).map(|n| n.clamp(PARALLEL_MIN as u64, PARALLEL_MAX as u64) as u8).unwrap_or(PARALLEL_DEFAULT);
    let mut rules: Vec<Rule> = Vec::new();
    match o.get("rules") {
        None => {}
        Some(Value::Array(items)) => {
            for it in items.iter().take(MAX_RULES) {
                match serde_json::from_value::<Rule>(it.clone()) {
                    Ok(mut r) => {
                        r.actions.truncate(MAX_ACTIONS);
                        r.timeout_secs = r.timeout_secs.clamp(TIMEOUT_MIN, TIMEOUT_MAX);
                        rules.push(r);
                    }
                    Err(_) => bad = true,
                }
            }
        }
        Some(_) => bad = true,
    }
    fix_ids(&mut rules);
    Parsed { config: Config { enabled, parallel, rules, extra }, bad, version }
}

/// Kennung 0 und doppelte Kennungen neu vergeben (die erste behält ihre).
pub fn fix_ids(list: &mut [Rule]) {
    let mut seen = std::collections::HashSet::new();
    let mut next = list.iter().map(|d| d.id).max().unwrap_or(0) + 1;
    for d in list.iter_mut() {
        if d.id == 0 || !seen.insert(d.id) {
            d.id = next;
            seen.insert(next);
            next += 1;
        }
    }
}

/// Was beim Laden über die Datei bekannt ist.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileMeta {
    /// Speichern gesperrt (Grund): unlesbar und nicht zu sichern, oder aus einer neueren Version.
    pub locked: Option<String>,
}

/// `save-actions.json` lesen (wie `processes.json`): fehlt die Datei, leere Liste; teilweise unlesbar → Sicherung
/// `save-actions.defekt-<zeit>.json`; nicht lesbar oder neuere Version → Speichern gesperrt (die Datei bleibt).
pub fn load(dir: &Path) -> (Config, FileMeta, Option<String>) {
    let path = dir.join(FILE);
    let de = crate::state::lang() == Lang::De;
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Config::default(), FileMeta::default(), None),
        Err(e) => {
            let msg = if de {
                format!("Die Aktionen beim Speichern (save-actions.json) ließen sich nicht lesen ({e}). Änderungen werden nicht gespeichert, damit die Datei erhalten bleibt – bitte prüfen und Paneldeck neu starten.")
            } else {
                format!("The actions on save (save-actions.json) could not be read ({e}). Changes are not saved so the file stays intact – please check it and restart Paneldeck.")
            };
            return (Config::default(), FileMeta { locked: Some(msg.clone()) }, Some(msg));
        }
    };
    let (text, bad_utf8) = match String::from_utf8(bytes) {
        Ok(t) => (t, false),
        Err(e) => (String::from_utf8_lossy(e.as_bytes()).into_owned(), true),
    };
    let p = parse(&text);
    let mut meta = FileMeta::default();
    let mut notice = None;
    if p.bad || bad_utf8 {
        match crate::state::backup_corrupt(&path) {
            Some(b) => {
                notice = Some(if de {
                    format!("Die Aktionen beim Speichern (save-actions.json) waren teilweise unlesbar – Sicherung: {}", b.display())
                } else {
                    format!("The actions on save (save-actions.json) were partly unreadable – backup: {}", b.display())
                });
            }
            None => {
                let msg = if de {
                    "Die Aktionen beim Speichern (save-actions.json) waren teilweise unlesbar und ließen sich nicht sichern. Änderungen werden nicht gespeichert."
                } else {
                    "The actions on save (save-actions.json) were partly unreadable and could not be backed up. Changes are not saved."
                };
                meta.locked = Some(msg.into());
                notice = Some(msg.into());
            }
        }
    }
    if p.version > FORMAT_VERSION as u64 {
        let msg = if de {
            format!("Die Aktionen beim Speichern (save-actions.json) stammen aus einer neueren Paneldeck-Version (Format {}). Änderungen werden nicht gespeichert, damit nichts verloren geht.", p.version)
        } else {
            format!("The actions on save (save-actions.json) come from a newer Paneldeck version (format {}). Changes are not saved so nothing gets lost.", p.version)
        };
        meta.locked = Some(msg.clone());
        notice = Some(msg);
    }
    (p.config, meta, notice)
}

/// Atomar speichern (gesperrt → Fehler mit Grund, Datei unverändert).
pub fn save(dir: &Path, cfg: &Config, meta: &FileMeta) -> std::io::Result<()> {
    if let Some(why) = &meta.locked {
        return Err(std::io::Error::other(why.clone()));
    }
    let text = serde_json::to_string_pretty(&cfg.to_json()).map_err(std::io::Error::other)?;
    crate::state::write_atomic(&dir.join(FILE), text.as_bytes())
}

/// Woher ein Speichern kommt (bestimmt, welche Regeln laufen).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SaveOrigin {
    /// Strg+S / *Speichern*.
    User,
    /// *Speichern unter*.
    SaveAs,
    /// *Alle speichern* (Menü *Datei* des Editors).
    SaveAll,
    /// Speichern in der Rückfrage beim Schließen.
    Close,
    /// Automatisches Speichern.
    Autosave,
    /// Agent (MCP `editor_save`, Chats).
    Agent,
}

impl SaveOrigin {
    /// Ausdrückliches Speichern durch den Nutzer: löst immer aus.
    pub fn explicit(self) -> bool {
        matches!(self, SaveOrigin::User | SaveOrigin::SaveAs | SaveOrigin::SaveAll | SaveOrigin::Close)
    }
}

/// Schlüssel eines Pfads zum Vergleichen (Windows: ohne Groß/Klein, `\`; ohne abschließenden Trenner).
pub fn path_key(p: &Path) -> String {
    let s = p.to_string_lossy();
    if cfg!(windows) {
        let s = s.replace('/', "\\");
        let s = s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s);
        let t = s.trim_end_matches('\\');
        // `C:\` bleibt `C:\` (sonst würde daraus ein relativer Laufwerkspfad).
        let t = if t.len() == 2 && t.ends_with(':') { s.as_str() } else { t };
        t.to_lowercase()
    } else {
        let t = s.trim_end_matches('/');
        if t.is_empty() { "/".into() } else { t.to_string() }
    }
}

/// Liegt `path` in `folder` (oder ist es)? Textvergleich wie [`path_key`].
pub fn is_inside(path: &Path, folder: &Path) -> bool {
    let p = path_key(path);
    let f = path_key(folder);
    if f.is_empty() {
        return false;
    }
    let sep = if cfg!(windows) { '\\' } else { '/' };
    p == f || (p.starts_with(&f) && (f.ends_with(sep) || p[f.len()..].starts_with(sep)))
}

/// Pfad relativ zu `folder` (Bestandteile), wenn `path` darin liegt.
pub fn relative(path: &Path, folder: &Path) -> Option<PathBuf> {
    if !is_inside(path, folder) {
        return None;
    }
    let n = path.components().count();
    let f = folder.components().count();
    if n <= f {
        return Some(PathBuf::new());
    }
    Some(path.components().skip(f).collect())
}

/// Zeit jetzt (Sekunden seit 1970).
pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> &'static str {
        if cfg!(windows) { r"C:\work\app" } else { "/work/app" }
    }

    #[test]
    fn round_trip_keeps_fields_unknown_ones_and_actions() {
        let mut r = Rule::new(3, Scope::Pattern, root());
        r.name = "Lint".into();
        r.patterns = vec!["src/**/*.ts".into(), "!dist/**".into()];
        r.actions = vec![
            Action::new(ActionKind::Builtin(Builtin::TrimWhitespace)),
            Action::new(ActionKind::Builtin(Builtin::LineEndings(crate::editor::LineEnding::CrLf))),
            Action::new(ActionKind::Builtin(Builtin::Encoding(crate::editor::TextEncoding::Utf8Bom))),
            Action::new(ActionKind::Builtin(Builtin::TabsToSpaces(4))),
            Action { kind: ActionKind::Command { command: "npx eslint --fix ${file}".into() }, continue_on_error: true },
            Action::new(ActionKind::Copy { target: "D:\\backup".into(), keep_structure: false }),
            Action::new(ActionKind::SendToSession { session: 7, name: "Build".into(), text: "make".into() }),
            Action::new(ActionKind::RestartProcess { process: 2, group: String::new(), name: "web".into() }),
            Action::new(ActionKind::RestartProcess { process: 0, group: "Backend".into(), name: "Backend".into() }),
            Action::new(ActionKind::Notify { text: "Saved ${fileBasename}".into() }),
            Action::new(ActionKind::Unknown(json!({"kind": "upload_magic", "x": 1}))),
        ];
        r.on_autosave = true;
        r.timeout_secs = 120;
        r.extra.insert("future_field".into(), json!({"a": 1}));
        let mut cfg = Config { rules: vec![r.clone()], ..Default::default() };
        cfg.extra.insert("future_top".into(), json!(true));
        let text = serde_json::to_string(&cfg.to_json()).unwrap();
        let back = parse(&text);
        assert!(!back.bad);
        assert_eq!(back.config, cfg);
        // Standardwerte werden nicht geschrieben.
        let minimal = serde_json::to_value(Rule::new(1, Scope::Folder, root())).unwrap();
        let keys: Vec<&String> = minimal.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["actions", "id", "name", "path", "scope"]);
    }

    #[test]
    fn missing_or_old_files_load_and_bad_entries_are_skipped() {
        let dir = crate::editor::test_dir("sa-load");
        let (cfg, meta, note) = load(&dir);
        assert_eq!(cfg, Config::default());
        assert!(meta.locked.is_none() && note.is_none());
        // BOM, fehlende Felder, ein kaputter Eintrag, doppelte Kennungen.
        let text = "\u{feff}{\"rules\":[{\"id\":1,\"path\":\"x\"},{\"id\":\"bad\"},{\"id\":1,\"scope\":\"file\",\"path\":\"y\",\"timeout_secs\":99999}]}";
        std::fs::write(dir.join(FILE), text).unwrap();
        let (cfg, meta, note) = load(&dir);
        assert_eq!(cfg.rules.len(), 2);
        assert_eq!(cfg.rules[1].id, 2);
        assert_eq!(cfg.rules[1].timeout_secs, TIMEOUT_MAX);
        assert!(cfg.enabled && cfg.parallel == PARALLEL_DEFAULT);
        assert!(meta.locked.is_none());
        assert!(note.is_some(), "Sicherung gemeldet");
        // Neuere Version: nie überschreiben.
        std::fs::write(dir.join(FILE), "{\"version\":9,\"rules\":[]}").unwrap();
        let (cfg, meta, _) = load(&dir);
        assert!(meta.locked.is_some());
        assert!(save(&dir, &cfg, &meta).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn paths_compare_and_relative() {
        let f = PathBuf::from(root());
        let inside = f.join("src").join("a.ts");
        assert!(is_inside(&inside, &f));
        assert!(!is_inside(&PathBuf::from(format!("{}x", root())).join("a.ts"), &f), "Nachbarordner mit gleichem Anfang");
        assert_eq!(relative(&inside, &f).unwrap(), PathBuf::from("src").join("a.ts"));
        if cfg!(windows) {
            assert!(is_inside(Path::new(r"c:\WORK\App\x.txt"), &f), "Windows: ohne Groß/Klein");
        }
    }

    #[test]
    fn origins_explicit_or_not() {
        for o in [SaveOrigin::User, SaveOrigin::SaveAs, SaveOrigin::SaveAll, SaveOrigin::Close] {
            assert!(o.explicit());
        }
        assert!(!SaveOrigin::Autosave.explicit() && !SaveOrigin::Agent.explicit());
    }
}
