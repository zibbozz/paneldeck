//! Prozessliste (#243): lang laufende Programme wie Entwicklungsserver (`php -S`, `ng serve`, `npm run dev`) mit ihrer
//! Ausgabe in einem eigenen Bereich – getrennt von den interaktiven Sitzungen. Dieses Modul kennt kein egui und keine
//! App: Datenformat `processes.json` (öffentlich, nur additiv ändern), Zustandsmaschine (Start, Stopp mit Strg+C und
//! nach [`STOP_GRACE`] Beenden des ganzen Prozessbaums, Ende), Aufbau der Befehlszeile je Shell, Erkennung lokaler
//! Adressen und Fehlerzeilen in der Ausgabe, rotierende Protokolldatei. Laufzeit und Oberfläche: `app/procs_ui.rs`.
//!
//! **Umgebungsvariablen:** Werte stehen nur verschlüsselt in der Datei (DPAPI wie die API-Schlüssel, `llm::secret`),
//! nie in Protokollen oder über MCP; nur die Namen sind lesbar.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::shells::ShellSpec;
use crate::state::Lang;

/// Name der Datei im Datenordner.
pub const FILE: &str = "processes.json";
/// Version des Datenformats (Feld `version`); neuere Versionen fügen nur Felder hinzu.
pub const FORMAT_VERSION: u32 = 1;
/// Ausgabezeilen je Prozess (Verlauf der Emulation).
pub const OUTPUT_LINES: usize = 10_000;
/// Wartezeit nach Strg+C, bevor der ganze Prozessbaum beendet wird.
pub const STOP_GRACE: Duration = Duration::from_secs(5);
/// Nach dem Beenden des Baums höchstens so lange auf das Ende warten; danach gilt der Prozess als gestoppt.
pub const KILL_GRACE: Duration = Duration::from_secs(5);
/// Größe einer Protokolldatei, ab der sie rotiert wird.
pub const LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;
/// Rotierte Protokolle (`.1` … `.N`) neben der aktuellen Datei.
pub const LOG_KEEP: usize = 3;
/// Ordner der Protokolldateien im Datenordner.
pub const LOG_DIR: &str = "process-logs";
/// Höchstzahl gemerkter lokaler Adressen je Prozess.
pub const MAX_URLS: usize = 6;
/// Exit-Code „Befehl nicht gefunden“ von cmd und unserer PowerShell-Hülle.
pub const NOT_FOUND_WIN: i64 = 9009;
/// Umgebungsvariable mit der Befehlszeile für cmd (`cmd /c %…%`, umgeht das Quoting der Kommandozeile).
pub const ENV_COMMAND: &str = "PANELDECK_PROCESS_COMMAND";

/// Wann ein Eintrag von selbst startet.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Autostart {
    /// Nur von Hand (Standard).
    #[default]
    Never,
    /// Beim Start von Paneldeck.
    Paneldeck,
    /// Wenn sein Projekt geöffnet wird (auch beim Start, wenn es das aktive Projekt ist).
    Project,
}

/// Tolerant: unbekannte Werte (z. B. aus einer neueren Version) zählen als `never`.
impl<'de> Deserialize<'de> for Autostart {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        Ok(match v.as_str() {
            Some("paneldeck") => Autostart::Paneldeck,
            Some("project") => Autostart::Project,
            _ => Autostart::Never,
        })
    }
}

impl Autostart {
    pub const ALL: [Autostart; 3] = [Autostart::Never, Autostart::Paneldeck, Autostart::Project];

    pub fn as_str(self) -> &'static str {
        match self {
            Autostart::Never => "never",
            Autostart::Paneldeck => "paneldeck",
            Autostart::Project => "project",
        }
    }

    pub fn label(self, lang: Lang) -> &'static str {
        match self {
            Autostart::Never => lang.t("Nie (nur von Hand)", "Never (only by hand)"),
            Autostart::Paneldeck => lang.t("Mit Paneldeck", "With Paneldeck"),
            Autostart::Project => lang.t("Wenn das Projekt geöffnet wird", "When the project is opened"),
        }
    }
}

/// Eine Umgebungsvariable eines Eintrags: der Name im Klartext, der Wert nur als DPAPI-Blob (Base64).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EnvVar {
    pub name: String,
    /// Verschlüsselter Wert (`llm::secret::protect`); fehlt, wenn er sich nicht verschlüsseln ließ (dann ohne Wert).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_protected: Option<String>,
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}
fn is_zero_i(v: &i64) -> bool {
    *v == 0
}
fn is_false(v: &bool) -> bool {
    !*v
}
fn is_never(v: &Autostart) -> bool {
    *v == Autostart::Never
}

/// Ein Eintrag der Prozessliste (eine Zeile in `processes.json`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProcDef {
    /// Eindeutige Kennung (> 0), bleibt beim Umbenennen gleich.
    pub id: u64,
    pub name: String,
    /// Befehlszeile; läuft über die gewählte Shell (`shell`), z. B. `npm run dev`.
    pub command: String,
    /// Arbeitsordner (leer = Standardordner der Shell).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cwd: String,
    /// Kennung einer erkannten Shell (`powershell`, `pwsh`, `cmd`, `gitbash`, `wsl:<Distribution>` …); leer = PowerShell.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub shell: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<EnvVar>,
    /// Projekt (`state.json`, 0 = keins).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub project: u64,
    /// Gruppe (leer = keine): startet/stoppt gemeinsam.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub group: String,
    /// Reihenfolge in der Gruppe: kleiner startet zuerst (und stoppt zuletzt).
    #[serde(default, skip_serializing_if = "is_zero_i")]
    pub order: i64,
    #[serde(default, skip_serializing_if = "is_never")]
    pub autostart: Autostart,
    /// Ausgabe zusätzlich in eine rotierende Protokolldatei schreiben (`process-logs/`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub log_file: bool,
    /// Unbekannte Felder (aus neueren Versionen) bleiben beim Speichern erhalten.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ProcDef {
    pub fn new(id: u64, name: &str, command: &str) -> ProcDef {
        ProcDef {
            id,
            name: name.into(),
            command: command.into(),
            cwd: String::new(),
            shell: String::new(),
            env: Vec::new(),
            project: 0,
            group: String::new(),
            order: 0,
            autostart: Autostart::Never,
            log_file: false,
            extra: BTreeMap::new(),
        }
    }

    pub fn env_names(&self) -> Vec<String> {
        self.env.iter().map(|e| e.name.clone()).collect()
    }
}

/// Was beim Laden über die Datei selbst bekannt ist und beim Speichern gebraucht wird.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileMeta {
    /// Unbekannte Felder der obersten Ebene (aus neueren Versionen); bleiben beim Speichern erhalten.
    pub extra: serde_json::Map<String, Value>,
    /// Speichern gesperrt (Grund für den Nutzer): Die Datei ließ sich nicht lesen und nicht sichern, oder sie stammt
    /// aus einer neueren Version. Dann wird sie nie überschrieben.
    pub locked: Option<String>,
}

/// Datei `processes.json` lesen (tolerant): BOM erlaubt, ein ungültiger Eintrag wird übersprungen, doppelte oder
/// fehlende Kennungen neu vergeben. Fehlt die Datei: leere Liste. Gibt die Liste, Angaben zur Datei und ggf. einen
/// Hinweis zurück.
/// * Teilweise unlesbar (kaputtes JSON, ungültiges UTF-8) → Sicherung `processes.defekt-<zeit>.json`, gespeichert wird
///   erst, wenn die Sicherung existiert; ließ sie sich nicht anlegen, bleibt das Speichern gesperrt.
/// * Nicht lesbar (gesperrt, keine Berechtigung …) → leere Liste, Speichern gesperrt (die Datei bleibt unverändert).
/// * Neuere `version` als [`FORMAT_VERSION`] → Speichern gesperrt (kein Herabstufen).
pub fn load(dir: &Path) -> (Vec<ProcDef>, FileMeta, Option<String>) {
    let path = dir.join(FILE);
    let de = crate::state::lang() == Lang::De;
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Vec::new(), FileMeta::default(), None),
        Err(e) => {
            let msg = if de {
                format!("Die Prozessliste (processes.json) ließ sich nicht lesen ({e}). Änderungen an der Liste werden nicht gespeichert, damit die Datei erhalten bleibt – bitte prüfen und Paneldeck neu starten.")
            } else {
                format!("The process list (processes.json) could not be read ({e}). Changes to the list are not saved so the file stays intact – please check it and restart Paneldeck.")
            };
            return (Vec::new(), FileMeta { extra: Default::default(), locked: Some(msg.clone()) }, Some(msg));
        }
    };
    let (text, bad_utf8) = match String::from_utf8(bytes) {
        Ok(t) => (t, false),
        Err(e) => (String::from_utf8_lossy(e.as_bytes()).into_owned(), true),
    };
    let p = parse_file(&text);
    let mut meta = FileMeta { extra: p.extra, locked: None };
    let mut notice = None;
    if p.bad || bad_utf8 {
        match crate::state::backup_corrupt(&path) {
            Some(b) => {
                notice = Some(if de {
                    format!("Die Prozessliste (processes.json) war teilweise unlesbar – Sicherung: {}", b.display())
                } else {
                    format!("The process list (processes.json) was partly unreadable – backup: {}", b.display())
                });
            }
            None => {
                let msg = if de {
                    "Die Prozessliste (processes.json) war teilweise unlesbar und ließ sich nicht sichern. Änderungen werden nicht gespeichert, damit die Datei erhalten bleibt – bitte prüfen und Paneldeck neu starten."
                } else {
                    "The process list (processes.json) was partly unreadable and could not be backed up. Changes are not saved so the file stays intact – please check it and restart Paneldeck."
                };
                meta.locked = Some(msg.into());
                notice = Some(msg.into());
            }
        }
    }
    if p.version > FORMAT_VERSION as u64 {
        let msg = if de {
            format!("Die Prozessliste (processes.json) stammt aus einer neueren Paneldeck-Version (Format {}). Änderungen an der Liste werden nicht gespeichert, damit sie nicht verloren gehen.", p.version)
        } else {
            format!("The process list (processes.json) comes from a newer Paneldeck version (format {}). Changes to the list are not saved so nothing gets lost.", p.version)
        };
        meta.locked = Some(msg.clone());
        notice = Some(msg);
    }
    (p.list, meta, notice)
}

/// Zerlegter Inhalt von `processes.json`.
#[derive(Debug, Default)]
pub struct Parsed {
    pub list: Vec<ProcDef>,
    /// Etwas war unlesbar (kaputtes JSON, ungültiger Eintrag).
    pub bad: bool,
    /// Feld `version` (fehlt = 1).
    pub version: u64,
    /// Unbekannte Felder der obersten Ebene.
    pub extra: serde_json::Map<String, Value>,
}

/// Inhalt von `processes.json` zerlegen (mit Version und unbekannten Feldern der obersten Ebene).
pub fn parse_file(text: &str) -> Parsed {
    let Ok(v) = serde_json::from_str::<Value>(crate::state::strip_bom(text)) else { return Parsed { bad: true, version: 1, ..Default::default() } };
    let Some(obj) = v.as_object() else { return Parsed { bad: true, version: 1, ..Default::default() } };
    // Eine unlesbare Version zählt als neuer (sicherer: nicht überschreiben).
    let version = match obj.get("version") {
        None => 1,
        Some(x) => x.as_u64().unwrap_or(u64::MAX),
    };
    let extra: serde_json::Map<String, Value> = obj.iter().filter(|(k, _)| k.as_str() != "version" && k.as_str() != "processes").map(|(k, v)| (k.clone(), v.clone())).collect();
    let mut bad = false;
    let mut out: Vec<ProcDef> = Vec::new();
    match obj.get("processes") {
        None => {}
        Some(Value::Array(items)) => {
            for it in items {
                match serde_json::from_value::<ProcDef>(it.clone()) {
                    Ok(d) => out.push(d),
                    Err(_) => bad = true,
                }
            }
        }
        Some(_) => bad = true,
    }
    fix_ids(&mut out);
    Parsed { list: out, bad, version, extra }
}

/// Inhalt von `processes.json` zerlegen; `true` = etwas war unlesbar.
#[cfg(test)]
pub fn parse(text: &str) -> (Vec<ProcDef>, bool) {
    let p = parse_file(text);
    (p.list, p.bad)
}

/// Kennung 0 und doppelte Kennungen neu vergeben (die erste behält ihre).
pub fn fix_ids(list: &mut [ProcDef]) {
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

/// JSON der Datei (Version + Einträge).
#[cfg(test)]
pub fn to_json(list: &[ProcDef]) -> Value {
    to_json_with(list, &serde_json::Map::new())
}

/// JSON der Datei mit den unbekannten Feldern der obersten Ebene (`extra`, aus [`FileMeta`]).
pub fn to_json_with(list: &[ProcDef], extra: &serde_json::Map<String, Value>) -> Value {
    let mut m = extra.clone();
    m.insert("version".into(), serde_json::json!(FORMAT_VERSION));
    m.insert("processes".into(), serde_json::to_value(list).unwrap_or(Value::Array(Vec::new())));
    Value::Object(m)
}

/// Atomar speichern. Ist das Speichern gesperrt ([`FileMeta::locked`]), bleibt die Datei unverändert (Fehler mit dem
/// Grund).
pub fn save(dir: &Path, list: &[ProcDef], meta: &FileMeta) -> std::io::Result<()> {
    if let Some(why) = &meta.locked {
        return Err(std::io::Error::other(why.clone()));
    }
    let text = serde_json::to_string_pretty(&to_json_with(list, &meta.extra)).map_err(std::io::Error::other)?;
    crate::state::write_atomic(&dir.join(FILE), text.as_bytes())
}

/// Nächste freie Kennung.
pub fn next_id(list: &[ProcDef]) -> u64 {
    list.iter().map(|d| d.id).max().unwrap_or(0) + 1
}

/// Freier Name: `base`, sonst `base #2` …
pub fn unique_name(base: &str, list: &[ProcDef]) -> String {
    let taken = |n: &str| list.iter().any(|d| d.name.trim().eq_ignore_ascii_case(n.trim()));
    if !taken(base) {
        return base.to_string();
    }
    (2..).map(|i| format!("{base} #{i}")).find(|n| !taken(n)).unwrap_or_else(|| base.to_string())
}

/// Eintrag per Kennung (Zahl, auch als Text) oder Name finden (erst genau, dann ohne Groß-/Kleinschreibung).
pub fn find<'a>(list: &'a [ProcDef], key: &Value) -> Option<&'a ProcDef> {
    if let Some(id) = key.as_u64() {
        return list.iter().find(|d| d.id == id);
    }
    let s = key.as_str()?.trim();
    if let Ok(id) = s.parse::<u64>() {
        if let Some(d) = list.iter().find(|d| d.id == id) {
            return Some(d);
        }
    }
    list.iter().find(|d| d.name.trim() == s).or_else(|| list.iter().find(|d| d.name.trim().eq_ignore_ascii_case(s)))
}

/// Alle Gruppennamen (sortiert, ohne leere).
/// Ein Projekt wurde gelöscht (#386): seine Prozesse ziehen mit den Sitzungen nach `to`. Gibt zurück, ob sich etwas änderte.
pub fn move_project(list: &mut [ProcDef], from: u64, to: u64) -> bool {
    let mut changed = false;
    for d in list.iter_mut().filter(|d| d.project == from && from != 0) {
        d.project = to;
        changed = true;
    }
    changed
}

pub fn groups(list: &[ProcDef]) -> Vec<String> {
    let mut g: Vec<String> = list.iter().map(|d| d.group.trim().to_string()).filter(|g| !g.is_empty()).collect();
    g.sort_by_key(|s| s.to_lowercase());
    g.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    g
}

/// Mitglieder einer Gruppe (Groß-/Kleinschreibung egal) in Startreihenfolge: `order`, dann Name, dann Kennung.
/// Gestoppt wird in umgekehrter Reihenfolge.
pub fn group_members(list: &[ProcDef], group: &str) -> Vec<u64> {
    let g = group.trim();
    let mut m: Vec<&ProcDef> = list.iter().filter(|d| !g.is_empty() && d.group.trim().eq_ignore_ascii_case(g)).collect();
    m.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())).then(a.id.cmp(&b.id)));
    m.into_iter().map(|d| d.id).collect()
}

// ---------------------------------------------------------------------------------------------------------------
// Umgebungsvariablen

/// Ist `name` als Variablenname erlaubt? Nicht leer, ohne `=` und Steuerzeichen, keine Paneldeck-Variable (`PANELDECK_*`).
pub fn valid_env_name(name: &str) -> bool {
    let n = name.trim();
    !n.is_empty() && n.len() <= 255 && !n.contains('=') && !n.chars().any(|c| c.is_control() || c.is_whitespace()) && !crate::terminal::is_internal_env(n) && !n.to_ascii_uppercase().starts_with("PANELDECK_")
}

/// Wert für die Datei verschlüsseln. `None`, wenn das hier nicht geht (dann wird kein Wert gespeichert).
pub fn protect_value(value: &str) -> Option<String> {
    crate::llm::secret::protect(value).ok()
}

/// Werte entschlüsseln: (Name, Wert) der lesbaren und die Namen derer, die sich nicht entschlüsseln ließen oder keinen
/// Wert haben.
pub fn env_values(def: &ProcDef) -> (Vec<(String, String)>, Vec<String>) {
    let (mut ok, mut bad) = (Vec::new(), Vec::new());
    for e in &def.env {
        match e.value_protected.as_deref().map(crate::llm::secret::unprotect) {
            Some(Ok(v)) => ok.push((e.name.clone(), v)),
            _ => bad.push(e.name.clone()),
        }
    }
    (ok, bad)
}

// ---------------------------------------------------------------------------------------------------------------
// Befehlszeile je Shell

/// Art der Shell, die die Befehlszeile ausführt.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Flavor {
    /// PowerShell (Windows PowerShell oder PowerShell 7).
    Pwsh,
    Cmd,
    /// Git Bash / bash / zsh / sh unter Windows bzw. Unix.
    Bash,
    /// WSL (`wsl.exe -d <Distribution>`).
    Wsl,
}

pub fn flavor_of(shell: &ShellSpec) -> Flavor {
    if crate::shells::is_wsl(shell) {
        return Flavor::Wsl;
    }
    let stem = Path::new(&shell.program).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    match stem.as_str() {
        "pwsh" | "powershell" => Flavor::Pwsh,
        "cmd" => Flavor::Cmd,
        _ => Flavor::Bash,
    }
}

/// Shell eines Eintrags: die gewählte (`shell` = Kennung), leer = PowerShell 7, sonst Windows PowerShell, sonst die
/// erste erkannte (außerhalb von Windows). `None`: die gewählte Shell gibt es hier nicht.
pub fn resolve_shell<'a>(id: &str, shells: &'a [ShellSpec]) -> Option<&'a ShellSpec> {
    let id = id.trim();
    if id.is_empty() {
        return shells.iter().find(|s| s.id == "pwsh").or_else(|| shells.iter().find(|s| s.id == "powershell")).or_else(|| shells.first());
    }
    shells.iter().find(|s| s.id == id)
}

/// PowerShell-Hülle um die Befehlszeile: „nicht gefunden“ ergibt [`NOT_FOUND_WIN`], ein Fehler den Exit-Code des
/// Programms (sonst 1), Erfolg 0 – PowerShell selbst liefert bei `-Command` nicht verlässlich den Code des Programms.
pub fn pwsh_script(command: &str) -> String {
    format!(
        "$ErrorActionPreference='Continue'\ntry {{\n{command}\n$__pdOk=$?;$__pdLec=$global:LASTEXITCODE\n}} catch [System.Management.Automation.CommandNotFoundException] {{\n[Console]::Error.WriteLine($_.Exception.Message); exit {NOT_FOUND_WIN}\n}}\nif(-not $__pdOk){{ if($__pdLec){{ exit $__pdLec }}; exit 1 }}\nexit 0\n"
    )
}

/// UTF-16LE + Base64 für `-EncodedCommand`.
pub fn encode_pwsh(script: &str) -> String {
    let bytes: Vec<u8> = script.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    crate::mcp::base64(&bytes)
}

/// Programm und Argumente, die `command` über `shell` ausführen, plus zusätzliche Umgebungsvariablen.
/// * PowerShell: `-NoLogo -NoProfile -EncodedCommand <Hülle>` (kein Quoting der Kommandozeile nötig).
/// * cmd: `/d /c %PANELDECK_PROCESS_COMMAND%` mit der Befehlszeile in dieser Variable (cmd ersetzt sie vor dem
///   Zerlegen, Operatoren wie `&&` wirken also; Anführungszeichen bleiben unverändert).
/// * bash (Git Bash, Unix): `--login -c <befehl>` bzw. `-lc`; `CHERE_INVOKING=1` hält den Arbeitsordner.
/// * WSL: die Argumente der Shell (`-d <Distribution>`) + `-e bash -lc <befehl>`.
pub fn launch_spec(shell: &ShellSpec, command: &str) -> (ShellSpec, Vec<(String, String)>) {
    let flavor = flavor_of(shell);
    let mut spec = ShellSpec { id: shell.id.clone(), label: shell.label.clone(), program: shell.program.clone(), args: Vec::new() };
    let mut env = Vec::new();
    match flavor {
        Flavor::Pwsh => {
            spec.args = vec!["-NoLogo".into(), "-NoProfile".into(), "-EncodedCommand".into(), encode_pwsh(&pwsh_script(command))];
        }
        Flavor::Cmd => {
            // `%NAME%` in der Befehlszeile ersetzt die Laufzeit vorher ([`expand_cmd_vars`]), weil cmd den Inhalt
            // der Variable nicht ein zweites Mal ersetzt.
            spec.args = vec!["/d".into(), "/c".into(), format!("%{ENV_COMMAND}%")];
            env.push((ENV_COMMAND.to_string(), command.to_string()));
        }
        Flavor::Bash => {
            spec.args = if cfg!(windows) { vec!["--login".into(), "-c".into(), command.to_string()] } else { vec!["-lc".into(), command.to_string()] };
            env.push(("CHERE_INVOKING".into(), "1".into()));
        }
        Flavor::Wsl => {
            let mut a: Vec<String> = Vec::new();
            if let Some(d) = crate::shells::wsl_distro(&shell.args) {
                a.extend(["-d".to_string(), d]);
            }
            a.extend(["-e".into(), "bash".into(), "-lc".into(), command.to_string()]);
            spec.args = a;
        }
    }
    (spec, env)
}

/// cmd: `%NAME%` so ersetzen, wie cmd es bei einer getippten Befehlszeile tut (Phase 1, vor dem Zerlegen an `&&`, `|`
/// …), z. B. `cd %USERPROFILE%\app && npm start`. Nötig, weil cmd den per `%PANELDECK_PROCESS_COMMAND%` übergebenen
/// Text nur einmal ersetzt – Variablen darin blieben sonst wörtlich stehen. `lookup` liefert den Wert aus der Umgebung,
/// die das Programm bekommt (ohne Groß-/Kleinschreibung). Wie bei cmd auf der Befehlszeile: eine unbekannte Variable
/// bleibt samt `%` stehen, ein einzelnes `%` auch; Teilstring/Ersetzen (`%X:~0,3%`, `%X:a=b%`) wird nicht ausgewertet.
/// Ein Wert mit `&` wirkt danach wie in cmd als Operator – es ist dieselbe Umgebung, die der Nutzer selbst setzt.
pub fn expand_cmd_vars(command: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(command.len());
    let mut rest = command;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(j) = after.find('%') else {
            out.push('%');
            rest = after;
            continue;
        };
        let name = &after[..j];
        let ok = !name.is_empty() && !name.contains(':') && !name.chars().any(|c| c.is_control());
        match ok.then(|| lookup(name)).flatten() {
            Some(v) => {
                out.push_str(&v);
                rest = &after[j + 1..];
            }
            None => {
                // unbekannt: `%NAME` bleibt, die Suche geht am schließenden `%` weiter
                out.push('%');
                out.push_str(name);
                rest = &after[j..];
            }
        }
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------------------------------------------
// Zustand

/// Zustand eines Eintrags (MCP `status`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Gestoppt (vom Nutzer bzw. noch nicht gestartet).
    #[default]
    Stopped,
    Running,
    /// Strg+C gesendet, wartet auf das Ende (nach [`STOP_GRACE`] wird der Baum beendet).
    Stopping,
    /// Von selbst mit Exit-Code 0 beendet.
    Exited,
    /// Von selbst mit einem anderen Exit-Code beendet.
    Crashed,
    /// Start fehlgeschlagen (Befehl/Shell nicht gefunden, Ordner fehlt); ohne Exit-Code.
    Failed,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Stopped => "stopped",
            Status::Running => "running",
            Status::Stopping => "stopping",
            Status::Exited => "exited",
            Status::Crashed => "crashed",
            Status::Failed => "failed",
        }
    }

    /// Läuft (noch) ein Prozess?
    pub fn active(self) -> bool {
        matches!(self, Status::Running | Status::Stopping)
    }

    pub fn label(self, lang: Lang, code: Option<i64>) -> String {
        match self {
            Status::Stopped => lang.t("gestoppt", "stopped").into(),
            Status::Running => lang.t("läuft", "running").into(),
            Status::Stopping => lang.t("wird gestoppt …", "stopping …").into(),
            Status::Exited => lang.t("beendet", "exited").into(),
            Status::Crashed => match code {
                Some(c) => format!("{} ({} {c})", lang.t("abgestürzt", "crashed"), lang.t("Code", "code")),
                None => lang.t("abgestürzt", "crashed").into(),
            },
            Status::Failed => lang.t("Start fehlgeschlagen", "failed to start").into(),
        }
    }
}

/// Exit-Code als Zahl mit Vorzeichen (Windows meldet NTSTATUS-Werte wie 0xC000013A als u32).
pub fn signed_code(code: u32) -> i64 {
    if cfg!(windows) { code as i32 as i64 } else { code as i64 }
}

/// Zustand nach dem Ende ohne Stopp-Wunsch: 0 = beendet, „Befehl nicht gefunden“ der Shell = Start fehlgeschlagen
/// (cmd/PowerShell-Hülle 9009, bash 127/126), sonst abgestürzt. `None` (Code unbekannt) = abgestürzt.
pub fn classify_exit(flavor: Flavor, code: Option<i64>) -> Status {
    match code {
        Some(0) => Status::Exited,
        Some(c) if matches!(flavor, Flavor::Pwsh | Flavor::Cmd) && c == NOT_FOUND_WIN => Status::Failed,
        Some(127 | 126) if matches!(flavor, Flavor::Bash | Flavor::Wsl) => Status::Failed,
        _ => Status::Crashed,
    }
}

/// Was die Laufzeit nach einem Übergang tun soll.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Prozess starten (danach [`Life::on_spawned`]).
    Spawn,
    /// Strg+C an die Konsole senden.
    CtrlC,
    /// Ganzen Prozessbaum beenden (Job Object bzw. Prozessgruppe).
    KillTree,
    /// Unerwartet beendet: Benachrichtigung.
    Notify,
}

/// Zustandsmaschine eines Eintrags (ohne Prozess; die Laufzeit führt die [`Effect`]e aus).
#[derive(Clone, Debug, Default)]
pub struct Life {
    pub status: Status,
    /// Exit-Code des letzten Laufs (nicht bei „Start fehlgeschlagen“ ohne Code).
    pub exit_code: Option<i64>,
    /// Zeitpunkt des Strg+C (Status `Stopping`).
    pub stop_at: Option<Instant>,
    /// Baum schon beendet (nach [`STOP_GRACE`]).
    pub killed: bool,
    /// Nach dem Stoppen wieder starten (Neu starten).
    pub restart: bool,
    /// Grund für „Start fehlgeschlagen“, ohne Prozess (z. B. Ordner fehlt).
    pub message: Option<String>,
}

impl Life {
    /// Starten: aus jedem ruhenden Zustand; läuft er noch, nichts; wird er gestoppt, danach neu starten.
    pub fn start(&mut self) -> Vec<Effect> {
        match self.status {
            Status::Running => vec![],
            Status::Stopping => {
                self.restart = true;
                vec![]
            }
            _ => vec![Effect::Spawn],
        }
    }

    /// Ergebnis des Starts.
    pub fn on_spawned(&mut self, res: Result<(), String>) {
        self.stop_at = None;
        self.killed = false;
        self.restart = false;
        match res {
            Ok(()) => {
                self.status = Status::Running;
                self.exit_code = None;
                self.message = None;
            }
            Err(m) => {
                self.status = Status::Failed;
                self.exit_code = None;
                self.message = Some(m);
            }
        }
    }

    /// Stoppen: Strg+C, Status `Stopping`. Ein wartender Neustart entfällt.
    pub fn stop(&mut self, now: Instant) -> Vec<Effect> {
        self.restart = false;
        match self.status {
            Status::Running => {
                self.status = Status::Stopping;
                self.stop_at = Some(now);
                self.killed = false;
                vec![Effect::CtrlC]
            }
            _ => vec![],
        }
    }

    /// Neu starten: läuft er, stoppen und danach starten; sonst starten.
    pub fn restart(&mut self, now: Instant) -> Vec<Effect> {
        match self.status {
            Status::Running => {
                let fx = self.stop(now);
                self.restart = true;
                fx
            }
            Status::Stopping => {
                self.restart = true;
                vec![]
            }
            _ => vec![Effect::Spawn],
        }
    }

    /// Zeit weiter: nach [`STOP_GRACE`] den Baum beenden; endet er auch danach nicht ([`KILL_GRACE`]), gilt er als
    /// gestoppt (z. B. ein Prozess, der sich nicht beenden lässt).
    pub fn tick(&mut self, now: Instant) -> Vec<Effect> {
        let (Status::Stopping, Some(at)) = (self.status, self.stop_at) else { return vec![] };
        let gone = now.saturating_duration_since(at);
        if !self.killed && gone >= STOP_GRACE {
            self.killed = true;
            return vec![Effect::KillTree];
        }
        if self.killed && gone >= STOP_GRACE + KILL_GRACE {
            return self.on_exit(Flavor::Bash, None);
        }
        vec![]
    }

    /// Der Prozess ist beendet (`code` = Exit-Code, `None` unbekannt).
    pub fn on_exit(&mut self, flavor: Flavor, code: Option<i64>) -> Vec<Effect> {
        if !self.status.active() {
            return vec![];
        }
        let stopping = self.status == Status::Stopping;
        self.exit_code = code;
        self.stop_at = None;
        self.killed = false;
        if stopping {
            self.status = Status::Stopped;
            if std::mem::take(&mut self.restart) {
                return vec![Effect::Spawn];
            }
            return vec![];
        }
        self.status = classify_exit(flavor, code);
        if self.status == Status::Failed {
            self.exit_code = None;
        }
        match self.status {
            Status::Crashed | Status::Failed => vec![Effect::Notify],
            _ => vec![],
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Ausgabe: Adressen und Fehlerzeilen

/// URLs (`http://`, `https://`) in einer Ausgabezeile, ohne Satzzeichen am Ende.
pub fn find_urls(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let lower = line.to_ascii_lowercase();
    let mut from = 0;
    while let Some(pos) = ["http://", "https://"].iter().filter_map(|p| lower[from..].find(p).map(|i| i + from)).min() {
        let rest = &line[pos..];
        let end = rest.find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '`' | '|' | '\u{1b}')).unwrap_or(rest.len());
        let mut url = rest[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}']).to_string();
        // eine öffnende Klammer im Pfad behält ihre schließende
        if rest[..end].ends_with(')') && url.contains('(') && !url.ends_with(')') {
            url.push(')');
        }
        if url.len() > "https://".len() {
            out.push(url);
        }
        from = pos + end.max(1);
        if from >= line.len() {
            break;
        }
    }
    out
}

/// Host einer URL (ohne Port, IPv6 ohne Klammern, klein geschrieben).
pub fn url_host(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let auth = rest.split(['/', '?', '#']).next()?;
    let auth = auth.rsplit('@').next()?; // Benutzerangaben nie als Host
    if let Some(v6) = auth.strip_prefix('[') {
        return Some(v6.split(']').next()?.to_ascii_lowercase());
    }
    let host = auth.split(':').next()?;
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// Lokale Adresse? `localhost` (auch `*.localhost`), 127.0.0.0/8, `0.0.0.0`, `::1`, `::` und die eigenen Adressen des
/// Rechners (`own`, z. B. die LAN-Adresse).
pub fn is_local_host(host: &str, own: &[std::net::IpAddr]) -> bool {
    let h = host.trim_end_matches('.');
    if h == "localhost" || h.ends_with(".localhost") {
        return true;
    }
    match h.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(a)) => a.is_loopback() || a.is_unspecified() || own.contains(&std::net::IpAddr::V4(a)),
        Ok(std::net::IpAddr::V6(a)) => a.is_loopback() || a.is_unspecified() || own.contains(&std::net::IpAddr::V6(a)),
        Err(_) => false,
    }
}

/// Lokale URLs einer Zeile (nur http/https, nur lokale Hosts).
pub fn local_urls(line: &str, own: &[std::net::IpAddr]) -> Vec<String> {
    find_urls(line).into_iter().filter(|u| url_host(u).is_some_and(|h| is_local_host(&h, own))).collect()
}

/// Was der Browser öffnet: `0.0.0.0` und `[::]` (lauscht auf allen Adressen) werden zu `localhost`.
pub fn browser_url(url: &str) -> String {
    for (from, to) in [("://0.0.0.0", "://localhost"), ("://[::]", "://localhost")] {
        if let Some(i) = url.find(from) {
            return format!("{}{}{}", &url[..i], to, &url[i + from.len()..]);
        }
    }
    url.to_string()
}

/// URL in die Liste aufnehmen (ohne Doppelte, höchstens [`MAX_URLS`], neueste zuletzt).
pub fn remember_url(list: &mut Vec<String>, url: String) {
    if list.contains(&url) {
        return;
    }
    list.push(url);
    if list.len() > MAX_URLS {
        list.remove(0);
    }
}

/// Eigene Adressen des Rechners (für „lokal“): die Adresse, über die er ins Netz geht (UDP-Verbindung ohne Senden).
pub fn own_addrs() -> Vec<std::net::IpAddr> {
    let mut out = Vec::new();
    for (bind, target) in [("0.0.0.0:0", "198.51.100.1:9"), ("[::]:0", "[2001:db8::1]:9")] {
        if let Ok(s) = std::net::UdpSocket::bind(bind) {
            if s.connect(target).is_ok() {
                if let Ok(a) = s.local_addr() {
                    if !a.ip().is_unspecified() {
                        out.push(a.ip());
                    }
                }
            }
        }
    }
    out
}

/// Sieht die Zeile wie ein Fehler aus? Wörter wie `error`, `ERR!`, `fatal`, `exception`, `panicked`, `Traceback`,
/// aber nicht „0 errors“, „no errors“, „without errors“ oder „errors: 0“.
pub fn is_error_line(line: &str) -> bool {
    let l = line.to_lowercase();
    if l.trim().is_empty() {
        return false;
    }
    for ok in ["0 error", "no error", "without error", "errors: 0", "error(s): 0", "0 problems"] {
        if l.contains(ok) {
            return false;
        }
    }
    let word = |w: &str| -> bool {
        let mut from = 0;
        while let Some(i) = l[from..].find(w) {
            let at = from + i;
            let before = l[..at].chars().next_back();
            let after = l[at + w.len()..].chars().next();
            if !before.is_some_and(|c| c.is_alphanumeric()) && !after.is_some_and(|c| c.is_alphabetic() && c != 's') {
                return true;
            }
            from = at + w.len();
        }
        false
    };
    word("error") || l.contains("error:") || l.contains("err!") || word("fatal") || word("exception") || l.contains("panicked at") || l.contains("traceback (most recent call last)") || l.contains("unhandled rejection")
}

// ---------------------------------------------------------------------------------------------------------------
// Protokolldatei

/// Dateiname der Protokolldatei eines Eintrags: `<id>-<name>.log` (Name auf sichere Zeichen gekürzt).
pub fn log_name(id: u64, name: &str) -> String {
    let safe: String = name.chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' }).take(40).collect();
    let safe = safe.trim_matches(['.', '_']);
    if safe.is_empty() { format!("{id}.log") } else { format!("{id}-{safe}.log") }
}

/// Rotierende Protokolldatei: ab [`LOG_MAX_BYTES`] wird `x.log` zu `x.log.1` (…`.N`, [`LOG_KEEP`] Stück).
pub struct LogFile {
    path: PathBuf,
    max: u64,
    keep: usize,
    size: u64,
}

impl LogFile {
    pub fn new(path: PathBuf) -> LogFile {
        Self::with_limits(path, LOG_MAX_BYTES, LOG_KEEP)
    }

    pub fn with_limits(path: PathBuf, max: u64, keep: usize) -> LogFile {
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        LogFile { path, max, keep, size }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn rotate(&mut self) {
        let name = |n: usize| {
            let mut p = self.path.as_os_str().to_os_string();
            p.push(format!(".{n}"));
            PathBuf::from(p)
        };
        let _ = std::fs::remove_file(name(self.keep));
        for n in (1..self.keep).rev() {
            let _ = std::fs::rename(name(n), name(n + 1));
        }
        let _ = std::fs::rename(&self.path, name(1));
        self.size = 0;
    }

    /// Zeilen anhängen (mit Uhrzeit davor); rotiert vorher, wenn die Datei voll ist.
    pub fn write_lines(&mut self, lines: &[String]) {
        use std::io::Write;
        if lines.is_empty() {
            return;
        }
        let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let mut text = String::new();
        for l in lines {
            text.push_str(&stamp);
            text.push(' ');
            text.push_str(l);
            text.push('\n');
        }
        if self.size > 0 && self.size + text.len() as u64 > self.max {
            self.rotate();
        }
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.path) {
            if f.write_all(text.as_bytes()).is_ok() {
                self.size += text.len() as u64;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(id: &str, program: &str, args: &[&str]) -> ShellSpec {
        ShellSpec { id: id.into(), label: id.into(), program: program.into(), args: args.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn schema_round_trip_keeps_fields_and_unknown_ones() {
        let mut d = ProcDef::new(3, "API", "php -S localhost:8000");
        d.cwd = r"C:\work\api".into();
        d.shell = "cmd".into();
        d.env = vec![EnvVar { name: "APP_ENV".into(), value_protected: Some("AAAA".into()) }];
        d.project = 2;
        d.group = "Backend".into();
        d.order = -1;
        d.autostart = Autostart::Project;
        d.log_file = true;
        d.extra.insert("future_field".into(), serde_json::json!({"x": 1}));
        let text = serde_json::to_string_pretty(&to_json(std::slice::from_ref(&d))).unwrap();
        assert!(text.contains("\"version\": 1") && text.contains("\"future_field\""), "{text}");
        assert!(text.contains("\"autostart\": \"project\""), "{text}");
        let (back, bad) = parse(&text);
        assert!(!bad);
        assert_eq!(back, vec![d]);
    }

    #[test]
    fn minimal_entries_omit_defaults_and_load_with_bom() {
        let d = ProcDef::new(1, "x", "y");
        let v = serde_json::to_value(&d).unwrap();
        let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
        assert_eq!(keys, vec!["command", "id", "name"]);
        let (l, bad) = parse("\u{feff}{\"version\":1,\"processes\":[{\"id\":1,\"name\":\"x\",\"command\":\"y\",\"autostart\":\"someday\"}]}");
        assert!(!bad);
        assert_eq!(l[0].autostart, Autostart::Never, "unbekannter Wert = never");
    }

    #[test]
    fn tolerant_parse_skips_invalid_and_fixes_ids() {
        let (l, bad) = parse(r#"{"processes":[{"id":5,"name":"a","command":"x"},{"name":7},{"id":5,"name":"b","command":"y"},{"id":0,"name":"c","command":"z"}]}"#);
        assert!(bad);
        assert_eq!(l.iter().map(|d| d.id).collect::<Vec<_>>(), vec![5, 6, 7]);
        assert_eq!(parse("kaputt").0.len(), 0);
        assert!(parse("kaputt").1);
        assert_eq!(parse("{}"), (vec![], false));
    }

    fn temp_dir(tag: &str) -> PathBuf {
        static C: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!("paneldeck-test-procs-{tag}-{}-{}", std::process::id(), C.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn backups(dir: &Path) -> usize {
        std::fs::read_dir(dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("processes.defekt-")).count()
    }

    #[test]
    fn processes_move_with_a_deleted_project() {
        let mut v = vec![ProcDef { project: 2, ..ProcDef::new(1, "a", "x") }, ProcDef { project: 3, ..ProcDef::new(2, "b", "x") }, ProcDef::new(3, "c", "x")];
        assert!(move_project(&mut v, 2, 5));
        assert_eq!(v.iter().map(|d| d.project).collect::<Vec<_>>(), vec![5, 3, 0]);
        assert!(!move_project(&mut v, 2, 5), "nichts mehr zu verschieben");
        assert!(!move_project(&mut v, 0, 5), "„Allgemein“ zieht nie um");
    }

    #[test]
    fn unknown_top_level_fields_are_kept_and_newer_versions_never_overwritten() {
        let dir = temp_dir("top");
        std::fs::write(dir.join(FILE), r#"{"version":1,"later":{"a":1},"processes":[{"id":1,"name":"x","command":"y"}]}"#).unwrap();
        let (mut l, meta, note) = load(&dir);
        assert!(note.is_none() && meta.locked.is_none());
        l.push(ProcDef::new(2, "z", "w"));
        save(&dir, &l, &meta).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(FILE)).unwrap()).unwrap();
        assert_eq!(v["later"], serde_json::json!({"a": 1}), "{v}");
        assert_eq!(v["version"], serde_json::json!(FORMAT_VERSION));
        assert_eq!(v["processes"].as_array().unwrap().len(), 2);

        let newer = r#"{"version":7,"processes":[{"id":1,"name":"x","command":"y"}]}"#;
        std::fs::write(dir.join(FILE), newer).unwrap();
        let (l, meta, note) = load(&dir);
        assert_eq!(l.len(), 1);
        assert!(meta.locked.is_some() && note.is_some());
        assert!(save(&dir, &l, &meta).is_err());
        assert_eq!(std::fs::read_to_string(dir.join(FILE)).unwrap(), newer, "nie herabgestuft");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_utf8_keeps_entries_and_backs_up_first() {
        let dir = temp_dir("utf8");
        let mut bytes = br#"{"version":1,"processes":[{"id":1,"name":"API "#.to_vec();
        bytes.push(0xff);
        bytes.extend_from_slice(br#"","command":"php -S localhost:8000"}]}"#);
        std::fs::write(dir.join(FILE), &bytes).unwrap();
        let (l, meta, note) = load(&dir);
        assert_eq!(l.len(), 1, "Einträge bleiben lesbar");
        assert_eq!(l[0].command, "php -S localhost:8000");
        assert!(note.is_some() && meta.locked.is_none());
        assert_eq!(backups(&dir), 1, "Sicherung vor dem nächsten Speichern");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_is_an_empty_list() {
        let dir = temp_dir("missing");
        let (l, meta, note) = load(&dir);
        assert!(l.is_empty() && note.is_none() && meta == FileMeta::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn unreadable_file_is_never_overwritten() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = temp_dir("locked");
        let text = r#"{"version":1,"processes":[{"id":1,"name":"x","command":"y"}]}"#;
        std::fs::write(dir.join(FILE), text).unwrap();
        // ohne Freigabe geöffnet: Lesen und Kopieren scheitern (Freigabeverletzung)
        let lock = std::fs::OpenOptions::new().read(true).share_mode(0).open(dir.join(FILE)).unwrap();
        let (l, meta, note) = load(&dir);
        assert!(l.is_empty());
        assert!(meta.locked.is_some() && note.is_some());
        assert!(save(&dir, &[ProcDef::new(1, "neu", "z")], &meta).is_err());
        drop(lock);
        assert_eq!(std::fs::read_to_string(dir.join(FILE)).unwrap(), text, "Datei unverändert");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cmd_variables_are_expanded_like_a_typed_command_line() {
        let env = |n: &str| match n.to_ascii_uppercase().as_str() {
            "USERPROFILE" => Some(r"C:\Users\a b".to_string()),
            "X" => Some("1".to_string()),
            _ => None,
        };
        assert_eq!(expand_cmd_vars(r"cd %USERPROFILE%\app && npm start", env), r"cd C:\Users\a b\app && npm start");
        assert_eq!(expand_cmd_vars("echo %userprofile%", env), r"echo C:\Users\a b");
        assert_eq!(expand_cmd_vars("echo %NOPE% 100% %X%", env), "echo %NOPE% 100% 1");
        assert_eq!(expand_cmd_vars("echo %NOPE%X%", env), "echo %NOPE1", "Suche geht am schließenden % weiter");
        assert_eq!(expand_cmd_vars("echo %X:~0,1% %% %", env), "echo %X:~0,1% %% %");
        assert_eq!(expand_cmd_vars("ohne", env), "ohne");
    }

    #[test]
    fn find_by_id_or_name_and_unique_names() {
        let l = vec![ProcDef::new(1, "API", "a"), ProcDef::new(12, "Web", "b")];
        assert_eq!(find(&l, &serde_json::json!(12)).unwrap().name, "Web");
        assert_eq!(find(&l, &serde_json::json!("12")).unwrap().name, "Web");
        assert_eq!(find(&l, &serde_json::json!("api")).unwrap().id, 1);
        assert!(find(&l, &serde_json::json!("nope")).is_none());
        assert_eq!(unique_name("API", &l), "API #2");
        assert_eq!(unique_name("New", &l), "New");
        assert_eq!(next_id(&l), 13);
    }

    #[test]
    fn groups_start_in_order_and_ignore_case() {
        let mut a = ProcDef::new(1, "Frontend", "x");
        a.group = "Dev".into();
        a.order = 2;
        let mut b = ProcDef::new(2, "Backend", "y");
        b.group = "dev".into();
        b.order = 1;
        let mut c = ProcDef::new(3, "DB", "z");
        c.group = "Dev".into();
        c.order = 1;
        let l = vec![a, b, c, ProcDef::new(4, "Solo", "w")];
        assert_eq!(group_members(&l, "DEV"), vec![2, 3, 1]);
        assert_eq!(groups(&l), vec!["Dev".to_string()]);
        assert!(group_members(&l, "").is_empty());
    }

    #[test]
    fn env_names_are_checked() {
        assert!(valid_env_name("APP_ENV"));
        for bad in ["", "A=B", "A B", "PANELDECK_SESSION", "paneldeck_token", "PANELDECK_PROCESS_COMMAND", "PANELDECK_DATA", "X\tY"] {
            assert!(!valid_env_name(bad), "{bad:?}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn env_values_are_encrypted_and_never_plain() {
        let mut d = ProcDef::new(1, "x", "y");
        d.env.push(EnvVar { name: "SECRET".into(), value_protected: protect_value("geheim-123") });
        d.env.push(EnvVar { name: "BROKEN".into(), value_protected: Some("AAAA".into()) });
        d.env.push(EnvVar { name: "EMPTY".into(), value_protected: None });
        let text = serde_json::to_string(&to_json(std::slice::from_ref(&d))).unwrap();
        assert!(!text.contains("geheim-123"), "{text}");
        let (ok, bad) = env_values(&d);
        assert_eq!(ok, vec![("SECRET".to_string(), "geheim-123".to_string())]);
        assert_eq!(bad, vec!["BROKEN".to_string(), "EMPTY".to_string()]);
    }

    #[test]
    fn default_shell_is_powershell() {
        let shells = vec![spec("powershell", "powershell.exe", &[]), spec("cmd", "cmd.exe", &[]), spec("pwsh", r"C:\p\pwsh.exe", &[])];
        assert_eq!(resolve_shell("", &shells).unwrap().id, "pwsh");
        assert_eq!(resolve_shell("", &shells[..2]).unwrap().id, "powershell");
        assert_eq!(resolve_shell("cmd", &shells).unwrap().id, "cmd");
        assert!(resolve_shell("gitbash", &shells).is_none());
    }

    #[test]
    fn command_lines_per_shell() {
        let (s, env) = launch_spec(&spec("powershell", "powershell.exe", &["-NoLogo"]), "npm run dev");
        assert_eq!(&s.args[..3], &["-NoLogo", "-NoProfile", "-EncodedCommand"]);
        assert!(env.is_empty());
        let raw = crate::llm::secret::unbase64(&s.args[3]).unwrap();
        let utf16: Vec<u16> = raw.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let script = String::from_utf16(&utf16).unwrap();
        assert!(script.contains("\nnpm run dev\n") && script.contains("exit 9009"), "{script}");

        let (s, env) = launch_spec(&spec("cmd", "cmd.exe", &[]), "echo \"a b\" && exit 3");
        assert_eq!(s.args, vec!["/d", "/c", "%PANELDECK_PROCESS_COMMAND%"]);
        assert_eq!(env, vec![(ENV_COMMAND.to_string(), "echo \"a b\" && exit 3".to_string())]);

        let (s, _) = launch_spec(&spec("wsl:Ubuntu", "wsl.exe", &["-d", "Ubuntu"]), "ls");
        assert_eq!(s.args, vec!["-d", "Ubuntu", "-e", "bash", "-lc", "ls"]);
        assert_eq!(flavor_of(&spec("gitbash", r"C:\Git\bin\bash.exe", &[])), Flavor::Bash);
        let (s, env) = launch_spec(&spec("gitbash", r"C:\Git\bin\bash.exe", &["--login", "-i"]), "ls");
        assert_eq!(s.args.last().unwrap(), "ls");
        assert!(env.iter().any(|(k, v)| k == "CHERE_INVOKING" && v == "1"));
    }

    #[test]
    fn exit_codes_are_classified() {
        assert_eq!(classify_exit(Flavor::Pwsh, Some(0)), Status::Exited);
        assert_eq!(classify_exit(Flavor::Pwsh, Some(9009)), Status::Failed);
        assert_eq!(classify_exit(Flavor::Cmd, Some(9009)), Status::Failed);
        assert_eq!(classify_exit(Flavor::Bash, Some(127)), Status::Failed);
        assert_eq!(classify_exit(Flavor::Wsl, Some(126)), Status::Failed);
        assert_eq!(classify_exit(Flavor::Pwsh, Some(127)), Status::Crashed);
        assert_eq!(classify_exit(Flavor::Bash, Some(1)), Status::Crashed);
        assert_eq!(classify_exit(Flavor::Bash, None), Status::Crashed);
        if cfg!(windows) {
            assert_eq!(signed_code(0xC000013A), -1073741510);
        }
        assert!(Status::Crashed.label(Lang::En, Some(3)).contains("crashed (code 3)"));
        assert!(Status::Crashed.label(Lang::De, Some(3)).contains("abgestürzt (Code 3)"));
    }

    #[test]
    fn state_machine_start_stop_crash() {
        let t0 = Instant::now();
        let mut l = Life::default();
        assert_eq!(l.status, Status::Stopped);
        assert_eq!(l.start(), vec![Effect::Spawn]);
        l.on_spawned(Ok(()));
        assert_eq!(l.status, Status::Running);
        assert_eq!(l.start(), vec![], "läuft schon");
        assert_eq!(l.on_exit(Flavor::Pwsh, Some(2)), vec![Effect::Notify]);
        assert_eq!((l.status, l.exit_code), (Status::Crashed, Some(2)));
        assert_eq!(l.on_exit(Flavor::Pwsh, Some(2)), vec![], "zweites Ende zählt nicht");
        assert_eq!(l.start(), vec![Effect::Spawn]);
        l.on_spawned(Ok(()));
        assert_eq!(l.exit_code, None);
        assert_eq!(l.on_exit(Flavor::Pwsh, Some(0)), vec![]);
        assert_eq!(l.status, Status::Exited);
        // Start fehlgeschlagen
        l.on_spawned(Err("Ordner fehlt".into()));
        assert_eq!((l.status, l.exit_code, l.message.as_deref()), (Status::Failed, None, Some("Ordner fehlt")));
        l.on_spawned(Ok(()));
        assert_eq!(l.on_exit(Flavor::Cmd, Some(9009)), vec![Effect::Notify]);
        assert_eq!((l.status, l.exit_code), (Status::Failed, None));
        // Stopp von außen gilt als gestoppt, auch mit Fehlercode
        l.on_spawned(Ok(()));
        assert_eq!(l.stop(t0), vec![Effect::CtrlC]);
        assert_eq!(l.status, Status::Stopping);
        assert_eq!(l.stop(t0), vec![], "nur einmal Strg+C");
        assert_eq!(l.on_exit(Flavor::Pwsh, Some(-1073741510)), vec![]);
        assert_eq!(l.status, Status::Stopped);
        assert_eq!(l.stop(t0), vec![]);
    }

    #[test]
    fn stop_escalates_to_killing_the_tree() {
        let t0 = Instant::now();
        let mut l = Life::default();
        l.on_spawned(Ok(()));
        l.stop(t0);
        assert_eq!(l.tick(t0 + Duration::from_secs(4)), vec![]);
        assert_eq!(l.tick(t0 + STOP_GRACE), vec![Effect::KillTree]);
        assert!(l.killed);
        assert_eq!(l.tick(t0 + STOP_GRACE + Duration::from_secs(1)), vec![], "nur einmal beenden");
        assert_eq!(l.status, Status::Stopping);
        assert_eq!(l.on_exit(Flavor::Pwsh, Some(1)), vec![]);
        assert_eq!(l.status, Status::Stopped);
        // lässt sich nicht beenden: nach KILL_GRACE trotzdem gestoppt
        l.on_spawned(Ok(()));
        l.stop(t0);
        l.tick(t0 + STOP_GRACE);
        assert_eq!(l.tick(t0 + STOP_GRACE + KILL_GRACE), vec![]);
        assert_eq!(l.status, Status::Stopped);
        assert_eq!(l.tick(t0 + STOP_GRACE * 10), vec![], "ruhend: nichts mehr");
    }

    #[test]
    fn restart_stops_first_then_spawns() {
        let t0 = Instant::now();
        let mut l = Life::default();
        assert_eq!(l.restart(t0), vec![Effect::Spawn], "gestoppt: gleich starten");
        l.on_spawned(Ok(()));
        assert_eq!(l.restart(t0), vec![Effect::CtrlC]);
        assert!(l.restart && l.status == Status::Stopping);
        assert_eq!(l.on_exit(Flavor::Pwsh, Some(1)), vec![Effect::Spawn]);
        assert!(!l.restart);
        l.on_spawned(Ok(()));
        // Start während des Stoppens = Neustart danach; Stopp danach hebt ihn auf
        l.stop(t0);
        assert_eq!(l.start(), vec![]);
        assert!(l.restart);
        l.stop(t0);
        assert!(!l.restart);
        assert_eq!(l.on_exit(Flavor::Pwsh, None), vec![]);
    }

    #[test]
    fn urls_are_found_and_only_local_ones_kept() {
        let own: Vec<std::net::IpAddr> = vec!["192.168.1.20".parse().unwrap()];
        assert_eq!(find_urls("PHP 8.3 Development Server (http://localhost:8000) started"), vec!["http://localhost:8000"]);
        assert_eq!(find_urls("  ➜  Local:   http://localhost:5173/"), vec!["http://localhost:5173/"]);
        assert_eq!(find_urls("** Angular Live Development Server is listening on localhost:4200, open your browser on http://localhost:4200/ **"), vec!["http://localhost:4200/"]);
        assert_eq!(find_urls("see https://example.com/a_(b) and HTTP://127.0.0.1:3000."), vec!["https://example.com/a_(b)", "HTTP://127.0.0.1:3000"]);
        assert!(find_urls("http:// nothing").is_empty());
        for (line, want) in [
            ("http://localhost:4200/", true),
            ("http://app.localhost:3000", true),
            ("http://127.0.0.1:8000", true),
            ("http://127.5.5.5", true),
            ("http://0.0.0.0:8080", true),
            ("http://[::1]:5000/x", true),
            ("http://192.168.1.20:5173/", true),
            ("http://192.168.1.21:5173/", false),
            ("https://example.com", false),
            ("http://localhost.evil.com", false),
            ("http://user@evil.com", false),
            ("http://localhost@evil.com/", false),
        ] {
            assert_eq!(!local_urls(line, &own).is_empty(), want, "{line}");
        }
        assert_eq!(browser_url("http://0.0.0.0:8080/x"), "http://localhost:8080/x");
        assert_eq!(browser_url("http://[::]:8080"), "http://localhost:8080");
        assert_eq!(browser_url("http://localhost:1"), "http://localhost:1");
        let mut l = Vec::new();
        for i in 0..10 {
            remember_url(&mut l, format!("http://localhost:{i}"));
        }
        remember_url(&mut l, "http://localhost:9".into());
        assert_eq!(l.len(), MAX_URLS);
        assert_eq!(l.last().unwrap(), "http://localhost:9");
    }

    #[test]
    fn error_lines_are_detected() {
        for yes in ["ERROR in src/app.ts", "npm ERR! code ELIFECYCLE", "error: could not compile", "Fatal error: Uncaught Exception", "thread 'main' panicked at src/main.rs", "Traceback (most recent call last):", "[vite] Internal server error: x", "TypeError: undefined"] {
            assert!(is_error_line(yes), "{yes}");
        }
        for no in ["Compiled successfully, 0 errors", "No errors found.", "errors: 0, warnings: 2", "", "terrorist", "errorless", "Listening on 8000", "built without errors"] {
            assert!(!is_error_line(no), "{no}");
        }
    }

    #[test]
    fn log_file_rotates() {
        let dir = std::env::temp_dir().join(format!("pd-proclog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut lf = LogFile::with_limits(dir.join(log_name(4, "my api/dev")), 200, 2);
        assert!(lf.path().ends_with("4-my_api_dev.log"));
        for i in 0..20 {
            lf.write_lines(&[format!("line {i} with some text")]);
        }
        let p = lf.path().to_path_buf();
        let rot = |n: u32| PathBuf::from(format!("{}.{n}", p.display()));
        assert!(p.exists() && rot(1).exists() && rot(2).exists() && !rot(3).exists());
        assert!(std::fs::metadata(&p).unwrap().len() <= 200);
        assert!(std::fs::read_to_string(&p).unwrap().contains("line 19"));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(log_name(1, "///"), "1.log");
    }
}
