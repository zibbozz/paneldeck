//! WSL-Hilfen (#172): installierte Distributionen, Pfadumrechnung (`\\wsl.localhost\…` ↔ Linux, `C:\…` ↔ `/mnt/c/…`),
//! Prüfung von Linux-Pfaden und sichere `wsl.exe`-Aufrufe (Argumente einzeln, nie als Shell-Zeichenkette).
//!
//! Sicherheit: `wsl.exe` zerlegt seine Kommandozeile selbst und hält sich dabei nicht an die üblichen Windows-Regeln
//! (`\"` maskiert kein Anführungszeichen). Ein `"` in einem Pfad hinter `--cd` beendet den Wert, der Rest liefe als
//! Befehl in der Standard-Distribution. Deshalb lehnt [`validate_linux_path`] `"` und `\` ab, und jeder Linux-Pfad,
//! der eine Kommandozeile erreicht (`--cd`, `ls`, `test -d`), geht vorher durch diese Prüfung.
//!
//! Grenze: `C:\` ↔ `/mnt/c` setzt den Standard-Einhängepunkt von WSL voraus (`[automount] root = /mnt/` in
//! `/etc/wsl.conf`). Wer ihn umgestellt hat, bekommt `/mnt/<lw>`-Pfade, die in seiner Distribution nicht existieren
//! (die Ordnerprüfung meldet das dann).
//!
//! Alles hier ist ohne egui; die Oberfläche dazu ist `app::folder_picker`.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// Höchstlänge eines Linux-Pfads (PATH_MAX).
pub const MAX_PATH_LEN: usize = 4096;
/// Zuletzt benutzte WSL-Ordner: so viele je Distribution …
pub const RECENT_MAX: usize = 10;
/// … und so viele Distributionen.
pub const RECENT_DISTROS_MAX: usize = 16;

/// Zeitlimit für die Distributionsliste und das Home-Verzeichnis (der erste Aufruf startet ggf. die WSL-VM).
pub const SLOW_TIMEOUT: Duration = Duration::from_secs(15);
/// Zeitlimit für Vervollständigung und Ordnerprüfung.
pub const FAST_TIMEOUT: Duration = Duration::from_secs(5);
/// Mehr Ausgabe wird nicht gelesen (die Abfrage gilt dann als gescheitert).
const MAX_OUTPUT: usize = 1 << 20;

/// Ausgabe von `wsl.exe` dekodieren: die eigenen Meldungen (z. B. `-l -q`) kommen als UTF-16LE (mit oder ohne BOM),
/// mit `WSL_UTF8=1` als UTF-8. UTF-8-Text enthält nie ein Nullbyte, UTF-16 mit ASCII-Namen immer.
pub fn decode_output(bytes: &[u8]) -> String {
    let utf16 = bytes.starts_with(&[0xFF, 0xFE]) || bytes.contains(&0);
    let s = if utf16 {
        let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    s.replace('\u{feff}', "")
}

/// Namen aus `wsl.exe -l -q` (eine Distribution je Zeile).
pub fn parse_distro_list(bytes: &[u8]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in decode_output(bytes).lines() {
        let name = l.trim().trim_matches('\0').trim();
        if valid_distro(name) && !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
    }
    out
}

/// Standard-Distribution aus `wsl.exe -l -v` (Zeile mit `*` vorn; Spaltenköpfe sind übersetzt, `*` nicht).
/// `names` aus `-l -q` erlaubt Namen mit Leerzeichen; ohne Treffer zählt das erste Wort.
pub fn parse_default_distro(bytes: &[u8], names: &[String]) -> Option<String> {
    let text = decode_output(bytes);
    let line = text.lines().map(|l| l.trim_matches('\0').trim()).find(|l| l.starts_with('*'))?;
    let rest = line.trim_start_matches('*').trim_start();
    let hit = names
        .iter()
        .filter(|n| rest.starts_with(n.as_str()) && rest[n.len()..].chars().next().is_none_or(char::is_whitespace))
        .max_by_key(|n| n.len());
    match hit {
        Some(n) => Some(n.clone()),
        None => rest.split_whitespace().next().filter(|n| valid_distro(n)).map(str::to_string),
    }
}

/// Vorlagen und System-Distributionen (`*-Template`, `docker-desktop*`), die in der Auswahl nicht erscheinen.
pub fn is_hidden_distro(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    l.ends_with("-template") || l.starts_with("docker-desktop")
}

/// Liste für die Auswahl: ohne Vorlagen/System-Distributionen, die Standard-Distribution zuerst (ist sie
/// ausgeblendet, steht die erste sichtbare vorn).
pub fn visible_distros(names: Vec<String>, default: Option<&str>) -> Vec<String> {
    let mut v: Vec<String> = names.into_iter().filter(|n| !is_hidden_distro(n)).collect();
    if let Some(i) = default.and_then(|d| v.iter().position(|n| n == d)) {
        let d = v.remove(i);
        v.insert(0, d);
    }
    v
}

/// Plausibler Distributionsname für `wsl.exe -d <name>`: nicht leer, keine Steuerzeichen, keine Pfadtrenner oder
/// Anführungszeichen, beginnt nicht mit `-` (wäre eine Option).
pub fn valid_distro(name: &str) -> bool {
    !name.is_empty() && name.len() <= 128 && !name.starts_with('-') && !name.chars().any(|c| c.is_control() || matches!(c, '\\' | '/' | '"'))
}

/// Fehler bei einem Linux-Pfad.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathError {
    Empty,
    NotAbsolute,
    Control,
    TooLong,
    /// `"` oder `\` – `wsl.exe` würde den Pfad falsch zerlegen (Befehlseinschleusung über `--cd`).
    Quote,
}

impl PathError {
    /// Meldung in beiden Sprachen.
    pub fn message(self, de: bool) -> &'static str {
        match (self, de) {
            (PathError::Empty, true) => "Kein Pfad angegeben.",
            (PathError::Empty, false) => "No path given.",
            (PathError::NotAbsolute, true) => "Linux-Pfad muss mit / beginnen (oder ~ sein).",
            (PathError::NotAbsolute, false) => "A Linux path must start with / (or be ~).",
            (PathError::Control, true) => "Der Pfad enthält Steuerzeichen.",
            (PathError::Control, false) => "The path contains control characters.",
            (PathError::TooLong, true) => "Der Pfad ist zu lang (höchstens 4096 Bytes).",
            (PathError::TooLong, false) => "The path is too long (at most 4096 bytes).",
            (PathError::Quote, true) => "Der Pfad enthält \" oder \\ – das ist für WSL-Startordner nicht erlaubt.",
            (PathError::Quote, false) => "The path contains \" or \\ – not allowed for WSL start folders.",
        }
    }
}

/// Gültiger Startordner für `wsl.exe --cd`: absoluter POSIX-Pfad oder `~` (Home), ohne Steuerzeichen, ohne `"` und
/// `\` (siehe Modulbeschreibung), höchstens [`MAX_PATH_LEN`] Bytes. Ob der Ordner existiert, prüft [`Backend::is_dir`].
pub fn validate_linux_path(p: &str) -> Result<(), PathError> {
    if p.is_empty() {
        return Err(PathError::Empty);
    }
    if p.len() > MAX_PATH_LEN {
        return Err(PathError::TooLong);
    }
    if p.chars().any(char::is_control) {
        return Err(PathError::Control);
    }
    if p.contains(['"', '\\']) {
        return Err(PathError::Quote);
    }
    if p != "~" && !p.starts_with('/') {
        return Err(PathError::NotAbsolute);
    }
    Ok(())
}

/// Sieht der Text wie ein Linux-Pfad aus (`/…`, `~`, `~/…`)?
pub fn looks_linux(p: &str) -> bool {
    p.starts_with('/') || p == "~" || p.starts_with("~/")
}

/// Zeichen, die WSL im 9P-Share auf `U+F000 + Zeichen` abbildet (unter Windows in Namen verboten).
const PUA_MAPPED: &[char] = &['"', '*', ':', '<', '>', '?', '\\', '|'];

fn pua_to_char(c: char) -> char {
    match (c as u32).checked_sub(0xF000).and_then(char::from_u32) {
        Some(o) if (o as u32) < 0x20 || PUA_MAPPED.contains(&o) => o,
        _ => c,
    }
}

fn char_to_pua(c: char) -> char {
    if c != '\\' && c != '"' && PUA_MAPPED.contains(&c) { char::from_u32(0xF000 + c as u32).unwrap_or(c) } else { c }
}

/// Lokaler WSL-UNC-Pfad (`\\wsl.localhost\<Distro>\…`, `\\wsl$\<Distro>\…`, auch mit `/` oder `\\?\UNC\…`) →
/// (Distribution, Linux-Pfad). Zeichen aus dem 9P-Share (`U+F0xx`) werden zurückgebildet; das Ergebnis ist ungeprüft
/// (kann z. B. `"` enthalten – vor Gebrauch [`validate_linux_path`]). Andere Pfade: `None`.
pub fn unc_to_linux(p: &str) -> Option<(String, String)> {
    let s = p.replace('/', "\\");
    let lower = s.to_ascii_lowercase();
    // Präfixe sind ASCII: gleiche Bytelänge in `s` und `lower`.
    let rest_at = [r"\\?\unc\wsl.localhost\", r"\\?\unc\wsl$\", r"\\wsl.localhost\", r"\\wsl$\"]
        .iter()
        .find(|pre| lower.starts_with(*pre))
        .map(|pre| pre.len())?;
    let rest = &s[rest_at..];
    let (distro, tail) = rest.split_once('\\').unwrap_or((rest, ""));
    if !valid_distro(distro) {
        return None;
    }
    let parts: Vec<String> = tail.split('\\').filter(|c| !c.is_empty()).map(|c| c.chars().map(pua_to_char).collect()).collect();
    Some((distro.to_string(), format!("/{}", parts.join("/"))))
}

/// Ist das ein lokaler WSL-Pfad (`\\wsl.localhost\…`, `\\wsl$\…`)? Solche Pfade prüft der UI-Thread nie
/// (`is_dir`/`is_file` könnten die WSL-VM starten und blockieren); sie gelten als Ordner.
pub fn is_wsl_unc(p: &str) -> bool {
    unc_to_linux(p).is_some()
}

/// Linux-Pfad als Windows-Pfad über `\\wsl.localhost\<Distro>\…` (für den Ordnerdialog); unter Windows verbotene
/// Zeichen werden wie von WSL auf `U+F0xx` abgebildet.
pub fn linux_to_unc(distro: &str, linux: &str) -> String {
    let parts: Vec<String> = linux.split('/').filter(|c| !c.is_empty()).map(|c| c.chars().map(char_to_pua).collect()).collect();
    if parts.is_empty() {
        format!(r"\\wsl.localhost\{distro}\")
    } else {
        format!(r"\\wsl.localhost\{distro}\{}", parts.join("\\"))
    }
}

/// Windows-Pfad → Linux-Pfad in WSL: `C:\x` (auch `\\?\C:\x`) → `/mnt/c/x` (Standard-Einhängepunkt, siehe oben),
/// `\\wsl.localhost\D\x` → `/x`. Laufwerksrelative (`C:x`), relative Pfade und Netzwerkfreigaben: `None`.
pub fn windows_to_linux(p: &str) -> Option<String> {
    if let Some((_, l)) = unc_to_linux(p) {
        return Some(l);
    }
    let p = p.strip_prefix(r"\\?\").or_else(|| p.strip_prefix("//?/")).unwrap_or(p);
    let b = p.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        if b.len() > 2 && !matches!(b[2], b'\\' | b'/') {
            return None;
        }
        let parts: Vec<&str> = p[2..].split(['\\', '/']).filter(|c| !c.is_empty()).collect();
        let d = (b[0] as char).to_ascii_lowercase();
        return Some(if parts.is_empty() { format!("/mnt/{d}") } else { format!("/mnt/{d}/{}", parts.join("/")) });
    }
    None
}

/// Linux-Pfad → Windows-Pfad: `/mnt/c/x` → `C:\x`, sonst `\\wsl.localhost\<Distro>\…`. `~` und relative Pfade,
/// ohne Distribution auch alles außerhalb von `/mnt/<lw>`: `None`.
pub fn linux_to_windows(distro: &str, p: &str) -> Option<String> {
    if !p.starts_with('/') {
        return None;
    }
    let parts: Vec<&str> = p.split('/').filter(|c| !c.is_empty()).collect();
    if parts.len() >= 2 && parts[0] == "mnt" && parts[1].len() == 1 && parts[1].as_bytes()[0].is_ascii_alphabetic() {
        let d = parts[1].to_ascii_uppercase();
        return Some(format!(r"{d}:\{}", parts[2..].join("\\")));
    }
    if distro.is_empty() {
        return None;
    }
    Some(linux_to_unc(distro, p))
}

/// `~` / `~/x` mit dem Home-Verzeichnis auflösen; absolute Pfade bleiben, sonst `None`.
pub fn expand_home(p: &str, home: Option<&str>) -> Option<String> {
    if p.starts_with('/') {
        return Some(p.to_string());
    }
    let home = home.filter(|h| h.starts_with('/'))?;
    if p == "~" {
        return Some(home.to_string());
    }
    let rest = p.strip_prefix("~/")?;
    Some(format!("{}/{rest}", home.trim_end_matches('/')))
}

/// Pfad für die Ablage: abschließenden `/` entfernen (außer bei `/`), `~/x` mit bekanntem Home auflösen
/// (`wsl.exe --cd` kennt nur `~` allein).
pub fn normalize(p: &str, home: Option<&str>) -> String {
    let p = p.trim();
    let p = if p.len() > 1 { p.trim_end_matches('/') } else { p };
    let p = if p.is_empty() { "/" } else { p };
    if p.starts_with("~/") {
        if let Some(e) = expand_home(p, home) {
            return e;
        }
    }
    p.to_string()
}

/// Text im Eingabefeld → (Ordner, der aufgelistet wird, getippter Namensanfang). `/home/u/pr` → (`/home/u/`, `pr`),
/// `~/x` mit Home `/home/u` → (`/home/u/`, `x`). Ohne absoluten Pfad: `None`.
pub fn split_for_completion(text: &str, home: Option<&str>) -> Option<(String, String)> {
    let t = if text == "~" { "~/" } else { text };
    let full = expand_home(t, home)?;
    let cut = full.rfind('/')? + 1;
    Some((full[..cut].to_string(), full[cut..].to_string()))
}

/// Ausgabe von `ls -1 -p -A` → Unterordner (mit `/` am Ende), die mit `prefix` beginnen. Versteckte Ordner nur,
/// wenn der Anfang mit `.` beginnt. Namen, die als Startordner nicht erlaubt wären (Steuerzeichen, `"`, `\`),
/// fallen weg. Sortiert, höchstens `max`.
pub fn parse_ls(stdout: &[u8], prefix: &str, max: usize) -> Vec<String> {
    let text = String::from_utf8_lossy(stdout);
    let mut v: Vec<String> = text
        .lines()
        .filter(|l| l.ends_with('/') && l.len() > 1)
        .filter(|l| !l.chars().any(|c| c.is_control() || c == '"' || c == '\\'))
        .filter(|l| l.starts_with(prefix))
        .filter(|l| !l.starts_with('.') || prefix.starts_with('.'))
        .map(str::to_string)
        .collect();
    v.sort_by_key(|a| a.to_lowercase());
    v.truncate(max);
    v
}

/// `wsl.exe`-Argumente: Home-Verzeichnis ausgeben. Die Shell-Zeichenkette ist fest; die Distribution ist ein eigenes Argument.
pub fn home_args(distro: &str) -> Vec<String> {
    ["-d", distro, "-e", "sh", "-c", "printf '%s' \"$HOME\""].iter().map(|s| s.to_string()).collect()
}

/// `wsl.exe`-Argumente: Ordner auflisten (`-e` = ohne Shell). `dir` muss [`validate_linux_path`] bestehen.
pub fn list_args(distro: &str, dir: &str) -> Vec<String> {
    ["-d", distro, "-e", "ls", "-1", "-p", "-A", "--", dir].iter().map(|s| s.to_string()).collect()
}

/// `wsl.exe`-Argumente: existiert der Ordner? `dir` muss [`validate_linux_path`] bestehen.
pub fn isdir_args(distro: &str, dir: &str) -> Vec<String> {
    ["-d", distro, "-e", "test", "-d", dir].iter().map(|s| s.to_string()).collect()
}

/// Ergebnis von [`apply_cd`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cd {
    /// `--cd <Linux-Pfad>` ist gesetzt; der Windows-Startordner ist nur Platzhalter.
    Linux,
    /// Nichts geändert: Windows-Pfad (übersetzt `wsl.exe` selbst) oder eigenes `--cd` des Profils.
    Windows,
    /// Der Ordner liegt in einer anderen Distribution als die der Shell.
    OtherDistro(String),
    /// Ungültiger Linux-Pfad (nicht starten!).
    Invalid(PathError),
}

/// Startordner einer WSL-Sitzung: `--cd <pfad>` vor die Argumente setzen, wenn `cwd` ein Linux-Pfad ist (leer = `~`,
/// die Vorgabe aus #172) oder ein `\\wsl$\<distro>\…`-Pfad derselben Distribution. Pfade mit `"`/`\`/Steuerzeichen
/// kommen nie auf die Kommandozeile ([`Cd::Invalid`]). Eigene `--cd`-Angaben eines Profils bleiben unangetastet.
pub fn apply_cd(args: &mut Vec<String>, cwd: &str, distro: Option<&str>) -> Cd {
    if args.iter().any(|a| a == "--cd" || a == "~") {
        return Cd::Windows;
    }
    let cwd = cwd.trim();
    let target = if cwd.is_empty() || cwd.starts_with("~/") {
        // Ohne Home-Auflösung kennt `--cd` nur `~` allein.
        "~".to_string()
    } else if let Some((d, linux)) = unc_to_linux(cwd) {
        match distro {
            Some(x) if x.eq_ignore_ascii_case(&d) => linux,
            Some(_) => return Cd::OtherDistro(d),
            None => return Cd::Windows,
        }
    } else if looks_linux(cwd) {
        cwd.to_string()
    } else {
        return Cd::Windows;
    };
    if let Err(e) = validate_linux_path(&target) {
        return Cd::Invalid(e);
    }
    args.splice(0..0, ["--cd".to_string(), target]);
    Cd::Linux
}

/// Ordner vorn in die Liste der zuletzt benutzten WSL-Ordner der Distribution setzen (ohne Doppel, begrenzt).
pub fn push_recent(map: &mut BTreeMap<String, Vec<String>>, distro: &str, path: &str) {
    if !valid_distro(distro) || validate_linux_path(path).is_err() {
        return;
    }
    let key = map.keys().find(|k| k.eq_ignore_ascii_case(distro)).cloned().unwrap_or_else(|| distro.to_string());
    if !map.contains_key(&key) && map.len() >= RECENT_DISTROS_MAX {
        if let Some(k) = map.keys().next().cloned() {
            map.remove(&k);
        }
    }
    let v = map.entry(key).or_default();
    v.retain(|p| p != path);
    v.insert(0, path.to_string());
    v.truncate(RECENT_MAX);
}

/// Zuletzt benutzte Ordner einer Distribution (Groß-/Kleinschreibung des Namens egal).
pub fn recent_for<'a>(map: &'a BTreeMap<String, Vec<String>>, distro: &str) -> Option<&'a Vec<String>> {
    map.iter().find(|(k, _)| k.eq_ignore_ascii_case(distro)).map(|(_, v)| v)
}

/// Gelesene Liste bereinigen (settings.json ist eine Vertrauensgrenze): ungültige Einträge weg, Pfade normalisiert
/// und ohne Doppel, Distributionen ohne Rücksicht auf Groß-/Kleinschreibung zusammengelegt, Grenzen eingehalten.
pub fn sanitize_recent(map: &mut BTreeMap<String, Vec<String>>) {
    let old = std::mem::take(map);
    for (d, paths) in old {
        if !valid_distro(&d) {
            continue;
        }
        let key = map.keys().find(|k| k.eq_ignore_ascii_case(&d)).cloned().unwrap_or(d);
        let v = map.entry(key).or_default();
        for p in paths {
            if validate_linux_path(&p).is_err() {
                continue;
            }
            let p = normalize(&p, None);
            if validate_linux_path(&p).is_ok() && !v.contains(&p) {
                v.push(p);
            }
        }
        v.truncate(RECENT_MAX);
    }
    map.retain(|_, v| !v.is_empty());
    while map.len() > RECENT_DISTROS_MAX {
        let Some(k) = map.keys().next().cloned() else { break };
        map.remove(&k);
    }
}

/// Existiert ein Ordner? `Unknown`: keine Antwort (Zeitlimit, WSL-Fehler) – blockiert nichts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirCheck {
    Exists,
    Missing,
    Unknown,
}

/// Zugriff auf WSL – im Programm über `wsl.exe` ([`WslExe`]), in Tests ersetzbar. Alle Aufrufe blockieren und
/// laufen deshalb nur in Hintergrund-Threads.
pub trait Backend: Send + Sync {
    /// Distributionen für die Auswahl ([`visible_distros`]); leer, wenn keine oder die Abfrage scheiterte.
    fn distros(&self) -> Vec<String>;
    /// Die echte Standard-Distribution (die `wsl.exe` ohne `-d` startet) – auch wenn sie in der Auswahl
    /// ausgeblendet ist. `None`: unbekannt.
    fn default_distro(&self) -> Option<String>;
    /// Home-Verzeichnis des Standardnutzers.
    fn home(&self, distro: &str) -> Option<String>;
    /// Ausgabe von `ls -1 -p -A -- <dir>`; `None` = nicht lesbar (oder zu viel Ausgabe).
    fn list(&self, distro: &str, dir: &str) -> Option<Vec<u8>>;
    /// Existiert der Ordner?
    fn is_dir(&self, distro: &str, path: &str) -> DirCheck;
}

/// Echte Abfragen über `wsl.exe`; erfolgreiche Distributionslisten und Home-Verzeichnisse werden zwischengespeichert.
pub struct WslExe;

/// Zwischengespeichert: (sichtbare Distributionen, echte Standard-Distribution).
type DistroInfo = (Vec<String>, Option<String>);

fn distro_cache() -> &'static Mutex<Option<DistroInfo>> {
    static C: OnceLock<Mutex<Option<DistroInfo>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

/// Distributionen und Standard ermitteln; nur ein Erfolg wird zwischengespeichert (sonst beim nächsten Mal erneut).
fn distro_info() -> Option<DistroInfo> {
    if let Some(v) = distro_cache().lock().ok().and_then(|c| c.clone()) {
        return Some(v);
    }
    let out = run(&["-l".into(), "-q".into()], SLOW_TIMEOUT).filter(|o| o.code == Some(0) && !o.truncated)?;
    let names = parse_distro_list(&out.stdout);
    let default = run(&["-l".into(), "-v".into()], SLOW_TIMEOUT).and_then(|o| parse_default_distro(&o.stdout, &names));
    let info = (visible_distros(names, default.as_deref()), default);
    if let Ok(mut c) = distro_cache().lock() {
        *c = Some(info.clone());
    }
    Some(info)
}

fn home_cache() -> &'static Mutex<BTreeMap<String, String>> {
    static C: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(BTreeMap::new()))
}

impl Backend for WslExe {
    fn distros(&self) -> Vec<String> {
        distro_info().map(|i| i.0).unwrap_or_default()
    }

    fn default_distro(&self) -> Option<String> {
        distro_info().and_then(|i| i.1)
    }

    fn home(&self, distro: &str) -> Option<String> {
        if !valid_distro(distro) {
            return None;
        }
        if let Some(h) = home_cache().lock().ok().and_then(|c| c.get(distro).cloned()) {
            return Some(h);
        }
        let out = run(&home_args(distro), SLOW_TIMEOUT).filter(|o| o.code == Some(0) && !o.truncated)?;
        let h = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if validate_linux_path(&h).is_err() || h == "~" {
            return None;
        }
        if let Ok(mut c) = home_cache().lock() {
            c.insert(distro.to_string(), h.clone());
        }
        Some(h)
    }

    fn list(&self, distro: &str, dir: &str) -> Option<Vec<u8>> {
        if !valid_distro(distro) || !dir.starts_with('/') || validate_linux_path(dir).is_err() {
            return None;
        }
        run(&list_args(distro, dir), FAST_TIMEOUT).filter(|o| o.code == Some(0) && !o.truncated).map(|o| o.stdout)
    }

    fn is_dir(&self, distro: &str, path: &str) -> DirCheck {
        if !valid_distro(distro) || !path.starts_with('/') || validate_linux_path(path).is_err() {
            return DirCheck::Unknown;
        }
        match run(&isdir_args(distro, path), FAST_TIMEOUT).map(|o| o.code) {
            Some(Some(0)) => DirCheck::Exists,
            // `test -d` liefert 1 für „gibt es nicht“; andere Codes stammen von wsl.exe selbst (Distribution fehlt …).
            Some(Some(1)) => DirCheck::Missing,
            _ => DirCheck::Unknown,
        }
    }
}

/// Ergebnis eines `wsl.exe`-Aufrufs.
struct Output {
    code: Option<i32>,
    stdout: Vec<u8>,
    /// Mehr als [`MAX_OUTPUT`] Bytes (nicht vollständig gelesen).
    truncated: bool,
}

/// `wsl.exe` mit einzelnen Argumenten starten (kein Fenster, keine Eingabe), Ausgabe lesen (höchstens 1 MB);
/// nach `timeout` wird der Prozess beendet. `None` = nicht startbar oder Zeit überschritten.
#[cfg(windows)]
fn run(args: &[String], timeout: Duration) -> Option<Output> {
    use std::io::Read;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    let mut child = Command::new("wsl.exe")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = (&mut stdout).take(MAX_OUTPUT as u64 + 1).read_to_end(&mut buf);
        buf
    });
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(15)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let mut out = reader.join().unwrap_or_default();
    let truncated = out.len() > MAX_OUTPUT;
    out.truncate(MAX_OUTPUT);
    status.map(|s| Output { code: s.code(), stdout: out, truncated })
}

#[cfg(not(windows))]
fn run(_args: &[String], _timeout: Duration) -> Option<Output> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(s: &str, bom: bool) -> Vec<u8> {
        let mut v = if bom { vec![0xFF, 0xFE] } else { Vec::new() };
        for u in s.encode_utf16() {
            v.extend_from_slice(&u.to_le_bytes());
        }
        v
    }

    #[test]
    fn distro_list_utf16_bom_and_utf8() {
        let want = vec!["Ubuntu-24.04".to_string(), "Debian".to_string(), "docker-desktop".to_string()];
        let text = "Ubuntu-24.04\r\nDebian\r\n\r\ndocker-desktop\r\n";
        assert_eq!(parse_distro_list(&utf16(text, false)), want);
        assert_eq!(parse_distro_list(&utf16(text, true)), want);
        assert_eq!(parse_distro_list(text.as_bytes()), want, "WSL_UTF8=1");
        // BOM vor jeder Zeile, Nullbytes, Doppel und Unsinn (Option, Steuerzeichen) fallen weg.
        assert_eq!(parse_distro_list(&utf16("\u{feff}Ubuntu\n\u{feff}Ubuntu\n-x\n", false)), vec!["Ubuntu".to_string()]);
        assert!(parse_distro_list(&[]).is_empty());
        // Ungerade Länge (abgeschnitten): kein Absturz.
        let mut odd = utf16("Arch\n", true);
        odd.push(b'X');
        assert_eq!(parse_distro_list(&odd), vec!["Arch".to_string()]);
    }

    #[test]
    fn default_distro_from_verbose_list() {
        let names = vec!["Ubuntu".to_string(), "Ubuntu-24.04".to_string(), "My Distro".to_string()];
        let v = "  NAME            STATE           VERSION\r\n  Ubuntu          Stopped         2\r\n* Ubuntu-24.04    Running         2\r\n";
        assert_eq!(parse_default_distro(&utf16(v, true), &names).as_deref(), Some("Ubuntu-24.04"));
        // Deutsche Spaltenköpfe, Name mit Leerzeichen
        let v = "  NAME      STATUS   VERSION\n* My Distro Gestoppt 2\n  Ubuntu Gestoppt 2\n";
        assert_eq!(parse_default_distro(v.as_bytes(), &names).as_deref(), Some("My Distro"));
        // Kein Stern: keine Standard-Distribution bekannt
        assert_eq!(parse_default_distro(b"  NAME STATE\n  Ubuntu Running 2\n", &names), None);
        // Unbekannter Name: erstes Wort
        assert_eq!(parse_default_distro(b"* Arch Running 2\n", &names).as_deref(), Some("Arch"));
        let ordered = visible_distros(names.clone(), Some("Ubuntu-24.04"));
        assert_eq!(ordered, vec!["Ubuntu-24.04".to_string(), "Ubuntu".to_string(), "My Distro".to_string()]);
        assert_eq!(visible_distros(names.clone(), Some("Missing")), names);
    }

    #[test]
    fn templates_and_system_distros_are_hidden() {
        // So sieht es auf dem Entwicklungsrechner aus: Standard ist eine Vorlage.
        let names: Vec<String> = ["Ubuntu-Template", "Ubuntu-24.04", "docker-desktop", "docker-desktop-data", "archlinux"].iter().map(|s| s.to_string()).collect();
        let v = "  NAME                   STATE           VERSION\n* Ubuntu-Template        Stopped         2\n  Ubuntu-24.04           Running         2\n";
        let default = parse_default_distro(v.as_bytes(), &names);
        assert_eq!(default.as_deref(), Some("Ubuntu-Template"));
        assert_eq!(visible_distros(names.clone(), default.as_deref()), vec!["Ubuntu-24.04".to_string(), "archlinux".to_string()], "ausgeblendeter Standard: erste sichtbare vorn");
        assert_eq!(visible_distros(names, Some("archlinux")), vec!["archlinux".to_string(), "Ubuntu-24.04".to_string()]);
        assert!(is_hidden_distro("DOCKER-DESKTOP") && is_hidden_distro("debian-TEMPLATE"));
        assert!(!is_hidden_distro("Ubuntu") && !is_hidden_distro("templates"));
    }

    #[test]
    fn unc_to_linux_both_hosts_and_forms() {
        let l = |p: &str| unc_to_linux(p).map(|(d, p)| format!("{d}|{p}"));
        assert_eq!(l(r"\\wsl.localhost\Ubuntu-24.04\home\max\my project").as_deref(), Some("Ubuntu-24.04|/home/max/my project"));
        assert_eq!(l(r"\\wsl$\Ubuntu\home\max\").as_deref(), Some("Ubuntu|/home/max"));
        assert_eq!(l(r"\\WSL.LOCALHOST\Ubuntu").as_deref(), Some("Ubuntu|/"));
        assert_eq!(l(r"\\wsl$\Ubuntu\").as_deref(), Some("Ubuntu|/"));
        assert_eq!(l("//wsl.localhost/Debian/srv/a&b (1)/ä").as_deref(), Some("Debian|/srv/a&b (1)/ä"));
        assert_eq!(l(r"\\?\UNC\wsl.localhost\Ubuntu\tmp").as_deref(), Some("Ubuntu|/tmp"));
        assert_eq!(l(r"\\wsl.localhost\Ubuntu\a\\b").as_deref(), Some("Ubuntu|/a/b"));
        // 9P-Abbildung (U+F0xx) zurück: `a:b*c?` – und `"` kommt so zwar zurück, scheitert aber an der Prüfung.
        assert_eq!(l("\\\\wsl$\\Ubuntu\\a\u{F03A}b\u{F02A}c\u{F03F}").as_deref(), Some("Ubuntu|/a:b*c?"));
        let (_, q) = unc_to_linux("\\\\wsl$\\Ubuntu\\x\u{F022} y").unwrap();
        assert_eq!(validate_linux_path(&q), Err(PathError::Quote));
        assert_eq!(l("\\\\wsl$\\Ubuntu\\\u{F041}").as_deref(), Some("Ubuntu|/\u{F041}"), "andere PUA-Zeichen bleiben");
        // Keine WSL-Pfade
        assert_eq!(l(r"\\wsl.attacker.com\share\x"), None);
        assert_eq!(l(r"\\server\share"), None);
        assert_eq!(l(r"C:\Users\x"), None);
        assert_eq!(l(r"\\wsl$\"), None);
        assert_eq!(l(r"\\wsl$\-bad\x"), None);
        assert!(is_wsl_unc(r"\\wsl$\Ubuntu") && is_wsl_unc("//wsl.localhost/U/x"));
        assert!(!is_wsl_unc(r"C:\x") && !is_wsl_unc(r"\\server\s") && !is_wsl_unc("/home"));
    }

    #[test]
    fn windows_and_linux_round_trip() {
        assert_eq!(windows_to_linux(r"C:\Users\Max\a b").as_deref(), Some("/mnt/c/Users/Max/a b"));
        assert_eq!(windows_to_linux(r"d:\").as_deref(), Some("/mnt/d"));
        assert_eq!(windows_to_linux("E:").as_deref(), Some("/mnt/e"));
        assert_eq!(windows_to_linux(r"C:/x/y/").as_deref(), Some("/mnt/c/x/y"));
        assert_eq!(windows_to_linux(r"\\?\C:\x\y").as_deref(), Some("/mnt/c/x/y"));
        assert_eq!(windows_to_linux(r"C:\$Recycle.Bin\#1 'q'").as_deref(), Some("/mnt/c/$Recycle.Bin/#1 'q'"));
        assert_eq!(windows_to_linux(r"\\wsl$\Ubuntu\home\u").as_deref(), Some("/home/u"));
        assert_eq!(windows_to_linux("C:foo"), None, "laufwerksrelativ");
        assert_eq!(windows_to_linux(r"\\server\share"), None);
        assert_eq!(windows_to_linux("relative\\x"), None);
        assert_eq!(windows_to_linux("/home/u"), None);

        assert_eq!(linux_to_windows("Ubuntu", "/mnt/c/Users/Max/a b").as_deref(), Some(r"C:\Users\Max\a b"));
        assert_eq!(linux_to_windows("Ubuntu", "/mnt/d").as_deref(), Some(r"D:\"));
        assert_eq!(linux_to_windows("", "/mnt/d/x").as_deref(), Some(r"D:\x"));
        assert_eq!(linux_to_windows("Ubuntu", "/mnt/wsl/x").as_deref(), Some(r"\\wsl.localhost\Ubuntu\mnt\wsl\x"));
        assert_eq!(linux_to_windows("Ubuntu", "/home/u/p q").as_deref(), Some(r"\\wsl.localhost\Ubuntu\home\u\p q"));
        assert_eq!(linux_to_windows("Ubuntu", "~"), None);
        assert_eq!(linux_to_windows("", "/home/u"), None);
        assert_eq!(linux_to_unc("Ubuntu", "/"), r"\\wsl.localhost\Ubuntu\");
        assert_eq!(linux_to_unc("Ubuntu", "/a:b"), "\\\\wsl.localhost\\Ubuntu\\a\u{F03A}b");
        // Hin und zurück
        for p in ["/home/u/a b", "/", "/srv/ä-ö", "/x/a:b|c"] {
            assert_eq!(unc_to_linux(&linux_to_unc("Deb", p)).map(|x| x.1).as_deref(), Some(p));
        }
        for p in [r"C:\Users\Max", r"Z:\a b\c"] {
            assert_eq!(linux_to_windows("D", &windows_to_linux(p).unwrap()).as_deref(), Some(p));
        }
    }

    #[test]
    fn linux_path_validation() {
        assert_eq!(validate_linux_path("/home/u/p"), Ok(()));
        assert_eq!(validate_linux_path("~"), Ok(()));
        assert_eq!(validate_linux_path("/a b/$(id)/`x`;'q'&|*"), Ok(()), "Shell-Zeichen sind als einzelnes Argument harmlos");
        // `"` und `\` zerlegt wsl.exe selbst: nie erlaubt (Einschleusung über --cd, im Review nachgewiesen).
        assert_eq!(validate_linux_path("/tmp/x\" echo INJECTED-$((6*7)); #"), Err(PathError::Quote));
        assert_eq!(validate_linux_path("/tmp/x\\"), Err(PathError::Quote));
        assert_eq!(validate_linux_path("/a\\\"b"), Err(PathError::Quote));
        assert_eq!(validate_linux_path(""), Err(PathError::Empty));
        assert_eq!(validate_linux_path("home/u"), Err(PathError::NotAbsolute));
        assert_eq!(validate_linux_path("~/x"), Err(PathError::NotAbsolute));
        assert_eq!(validate_linux_path("C:/x"), Err(PathError::NotAbsolute));
        assert_eq!(validate_linux_path(r"C:\x"), Err(PathError::Quote));
        assert_eq!(validate_linux_path("/a\nb"), Err(PathError::Control));
        assert_eq!(validate_linux_path("/a\0b"), Err(PathError::Control));
        assert_eq!(validate_linux_path(&format!("/{}", "a".repeat(MAX_PATH_LEN))), Err(PathError::TooLong));
        assert!(PathError::NotAbsolute.message(true).contains("Linux"));
        assert!(PathError::TooLong.message(false).contains("4096"));
        assert!(PathError::Quote.message(false).contains('"'));
    }

    #[test]
    fn home_expansion_and_normalizing() {
        assert_eq!(expand_home("~", Some("/home/u")).as_deref(), Some("/home/u"));
        assert_eq!(expand_home("~/p", Some("/home/u/")).as_deref(), Some("/home/u/p"));
        assert_eq!(expand_home("~/p", None), None);
        assert_eq!(expand_home("/x", None).as_deref(), Some("/x"));
        assert_eq!(expand_home("x", Some("/home/u")), None);
        assert_eq!(normalize("/home/u/p/", None), "/home/u/p");
        assert_eq!(normalize("/", None), "/");
        assert_eq!(normalize("~/p/", Some("/home/u")), "/home/u/p");
        assert_eq!(normalize("~", Some("/home/u")), "~");
        assert_eq!(normalize("~/p", None), "~/p");
    }

    #[test]
    fn completion_parsing() {
        assert_eq!(split_for_completion("/home/u/pr", None), Some(("/home/u/".into(), "pr".into())));
        assert_eq!(split_for_completion("/home/u/", None), Some(("/home/u/".into(), "".into())));
        assert_eq!(split_for_completion("/", None), Some(("/".into(), "".into())));
        assert_eq!(split_for_completion("~/pr", Some("/home/u")), Some(("/home/u/".into(), "pr".into())));
        assert_eq!(split_for_completion("~", Some("/home/u")), Some(("/home/u/".into(), "".into())));
        assert_eq!(split_for_completion("~", None), None);
        assert_eq!(split_for_completion("rel", None), None);
        let ls = b"project/\nproto/\nREADME.md\n.config/\nPrivate/\nlink\nweird\x1bname/\npr\xc3\xa4fix/\npr\"q\" ; id/\npr\\x/\n";
        assert_eq!(parse_ls(ls, "pr", 10), vec!["project/".to_string(), "proto/".to_string(), "präfix/".to_string()], "Namen mit \" oder \\ nie vorschlagen");
        assert_eq!(parse_ls(ls, "", 10), vec!["Private/".to_string(), "project/".to_string(), "proto/".to_string(), "präfix/".to_string()]);
        assert_eq!(parse_ls(ls, ".", 10), vec![".config/".to_string()]);
        assert_eq!(parse_ls(ls, "", 2).len(), 2);
        assert!(parse_ls(b"", "", 10).is_empty());
    }

    #[test]
    fn argv_keeps_user_text_as_single_arguments() {
        let evil = "/tmp/x; rm -rf ~ $(id) `id` 'q' && echo";
        assert_eq!(validate_linux_path(evil), Ok(()));
        let a = list_args("Ubuntu", evil);
        assert_eq!(a, vec!["-d", "Ubuntu", "-e", "ls", "-1", "-p", "-A", "--", evil]);
        // `-e` = ohne Shell; der Pfad ist genau ein Argument und steht hinter `--`.
        assert_eq!(a.iter().filter(|x| x.as_str() == evil).count(), 1);
        assert_eq!(isdir_args("Ubuntu", evil), vec!["-d", "Ubuntu", "-e", "test", "-d", evil]);
        let h = home_args("Ubuntu-24.04");
        assert_eq!(&h[..5], &["-d", "Ubuntu-24.04", "-e", "sh", "-c"]);
        assert!(!h[5].contains("Ubuntu"), "nichts Veränderliches in der Shell-Zeichenkette");
        // Pfade mit Anführungszeichen erreichen wsl.exe nie (WslExe prüft vor jedem Aufruf).
        assert_eq!(WslExe.is_dir("Ubuntu", "/tmp/x\" echo pwned; #"), DirCheck::Unknown);
        assert_eq!(WslExe.list("Ubuntu", "/tmp/x\" echo pwned; #/"), None);
        assert!(!valid_distro("-e"));
        assert!(!valid_distro("a\nb"));
        assert!(!valid_distro(r"..\x"));
        assert!(!valid_distro("U\" x"));
        assert!(valid_distro("Ubuntu-24.04"));
    }

    #[test]
    fn cd_argument_for_sessions() {
        let base = || vec!["-d".to_string(), "Ubuntu".to_string()];
        let mut a = base();
        assert_eq!(apply_cd(&mut a, "/home/u/my project", Some("Ubuntu")), Cd::Linux);
        assert_eq!(a, vec!["--cd", "/home/u/my project", "-d", "Ubuntu"]);
        let mut a = base();
        assert_eq!(apply_cd(&mut a, "", Some("Ubuntu")), Cd::Linux, "leer = Linux-Home");
        assert_eq!(a, vec!["--cd", "~", "-d", "Ubuntu"]);
        let mut a = vec![];
        assert_eq!(apply_cd(&mut a, "~/x", None), Cd::Linux);
        assert_eq!(a, vec!["--cd", "~"]);
        // Gespeicherter \\wsl$-Pfad derselben Distribution → --cd; andere Distribution → Fehler; ohne bekannte Distribution wie bisher
        let mut a = base();
        assert_eq!(apply_cd(&mut a, r"\\wsl$\ubuntu\srv\a b", Some("Ubuntu")), Cd::Linux);
        assert_eq!(a[..2], ["--cd", "/srv/a b"]);
        let mut a = base();
        assert_eq!(apply_cd(&mut a, r"\\wsl.localhost\Debian\srv", Some("Ubuntu")), Cd::OtherDistro("Debian".into()));
        assert_eq!(apply_cd(&mut a, r"\\wsl.localhost\Debian\srv", None), Cd::Windows);
        assert_eq!(a.len(), 2);
        // Windows-Pfad: wie bisher über den Windows-Startordner
        assert_eq!(apply_cd(&mut a, r"C:\Users\u", Some("Ubuntu")), Cd::Windows);
        assert_eq!(a.len(), 2);
        // Einschleusung (im Review nachgewiesen) und Steuerzeichen: nichts anfügen, nicht starten
        assert_eq!(apply_cd(&mut a, "/tmp/x\" echo INJECTED-$((6*7)); #", Some("Ubuntu")), Cd::Invalid(PathError::Quote));
        assert_eq!(apply_cd(&mut a, "\\\\wsl$\\Ubuntu\\x\u{F022} echo pwned", Some("Ubuntu")), Cd::Invalid(PathError::Quote));
        assert_eq!(apply_cd(&mut a, "/a\nb", Some("Ubuntu")), Cd::Invalid(PathError::Control));
        assert_eq!(a.len(), 2);
        let mut own = vec!["--cd".to_string(), "/srv".to_string()];
        assert_eq!(apply_cd(&mut own, "/home", None), Cd::Windows);
        assert_eq!(own, vec!["--cd", "/srv"]);
    }

    #[test]
    fn recent_folders_are_capped_and_deduplicated() {
        let mut m = BTreeMap::new();
        for i in 0..15 {
            push_recent(&mut m, "Ubuntu", &format!("/p/{i}"));
        }
        push_recent(&mut m, "ubuntu", "/p/5");
        let v = &m["Ubuntu"];
        assert_eq!(v.len(), RECENT_MAX);
        assert_eq!(v[0], "/p/5");
        assert_eq!(v.iter().filter(|p| *p == "/p/5").count(), 1);
        assert_eq!(recent_for(&m, "UBUNTU").map(|v| v.len()), Some(RECENT_MAX));
        push_recent(&mut m, "Ubuntu", "relative");
        push_recent(&mut m, "Ubuntu", "/q\"x");
        push_recent(&mut m, "-bad", "/x");
        assert_eq!(m.len(), 1);
        for i in 0..20 {
            push_recent(&mut m, &format!("D{i:02}"), "/x");
        }
        assert_eq!(m.len(), RECENT_DISTROS_MAX);
        assert_eq!(m["D19"], vec!["/x".to_string()]);
        let mut bad: BTreeMap<String, Vec<String>> = BTreeMap::new();
        bad.insert("Ubuntu".into(), vec!["/ok".into(), "nope".into(), "/a\u{7}".into(), "/ok/".into(), "/q\"".into()]);
        bad.insert("ubuntu".into(), vec!["/ok".into(), "/two".into()]);
        bad.insert("-x".into(), vec!["/ok".into()]);
        bad.insert("Empty".into(), vec!["rel".into()]);
        sanitize_recent(&mut bad);
        assert_eq!(bad.len(), 1, "{bad:?}");
        assert_eq!(bad["Ubuntu"], vec!["/ok".to_string(), "/two".to_string()]);
    }
}
