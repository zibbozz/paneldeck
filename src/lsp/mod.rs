//! Smart-Modus des Editors (#367, 3.3.0): Language-Server-Client mit Vervollständigen und Diagnosen, Server aus dem
//! PATH oder einem eigenen Pfad je Sprache, Projektwurzel, Grenzen, Absturz-Behandlung und vertrauenswürdige Ordner.
//! Kein egui – die Oberfläche (`app::editor_ui::smart`) fragt [`Servers::tick`] je Bild ab.
//!
//! Aufbau: [`proto`] (Rahmen, JSON-RPC, die nötigen LSP-Typen), [`client`] (ein Server: Prozess, Fäden, Dokumente),
//! [`servers`] (eingebaute Liste, PATH-Suche), [`trust`] (`trust.json`), hier die Einstellungen ([`SmartSettings`],
//! `settings.json` → `editor.smart`) und die Verwaltung aller Server ([`Servers`]).
//!
//! Nicht in 3.3.0 (Folge-Issue für 3.4.0): Hover, Signaturhilfe, Gehe zu Definition/Verweisen, Umbenennen, Formatieren
//! über den Server, Gliederung, Quick-Fixes, KI-Vervollständigung, MCP `get_diagnostics`/`fix_all`.

pub mod client;
pub mod proto;
pub mod servers;
pub mod trust;

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Grenzen und Schalter (`settings.json` → `editor.smart`, öffentliches Datenformat ab 3.3.0, nur additiv; alle
/// Felder optional, unlesbare Werte = Standard).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SmartSettings {
    /// Höchstzahl gleichzeitig laufender Server (der am längsten unbenutzte endet), Standard 6.
    pub max_servers: u32,
    /// Zeitgrenze einer Vervollständigung (ms), Standard 5000.
    pub completion_timeout_ms: u32,
    /// Diagnosen je Datei, Standard 1000.
    pub max_diagnostics: u32,
    /// Größte Nachricht eines Servers (MB), Standard 64.
    pub max_message_mb: u32,
    /// Smart-Modus nur bis zu dieser Dateigröße (MB), Standard 5; darüber Syntax-Modus mit Hinweis.
    pub max_file_mb: u32,
    /// Vervollständigung öffnet sich von selbst (nach Auslösezeichen und 2 Wortzeichen), Standard an.
    pub auto_complete: bool,
    /// Mitschnitt der Nachrichten im Protokoll (nur im Speicher), Standard aus.
    pub trace: bool,
    /// Ein Server ohne offenes Dokument endet nach so vielen Minuten, Standard 10 (Grenze aus dem Issue).
    pub idle_minutes: u32,
    /// Tipp-Pause, nach der Diagnosen wieder gezeigt werden (ms), Standard 500.
    pub typing_pause_ms: u32,
    /// Nach so vielen Abstürzen …, Standard 3 …
    pub crash_max: u32,
    /// … innerhalb so vieler Minuten ist der Smart-Modus für die Sprache bis zum Neustart aus (Hinweis mit *Erneut
    /// versuchen* und *Protokoll zeigen*), Standard 5.
    pub crash_window_minutes: u32,
    /// Argumente des eigenen Servers je Sprache.
    pub own_args: BTreeMap<String, Vec<String>>,
    /// `initializationOptions` je Sprache (beliebiges JSON, nur über `settings.json`).
    pub init_options: BTreeMap<String, Value>,
    /// Eigener Server je Sprache (absoluter Pfad, ohne Argumente).
    pub own_paths: BTreeMap<String, PathBuf>,
    /// Abgewählte eingebaute Server je Sprache (Kennungen aus [`servers::known`]).
    pub disabled: BTreeMap<String, Vec<String>>,
}

pub const MAX_SERVERS_RANGE: (u32, u32) = (1, 32);
pub const COMPLETION_TIMEOUT_RANGE: (u32, u32) = (500, 60_000);
pub const MAX_DIAGNOSTICS_RANGE: (u32, u32) = (10, 100_000);
pub const MAX_MESSAGE_MB_RANGE: (u32, u32) = (1, 1024);
pub const MAX_FILE_MB_RANGE: (u32, u32) = (1, 1024);
pub const IDLE_MINUTES_RANGE: (u32, u32) = (1, 1440);
pub const TYPING_PAUSE_RANGE: (u32, u32) = (50, 10_000);
pub const CRASH_MAX_RANGE: (u32, u32) = (1, 100);
pub const CRASH_WINDOW_RANGE: (u32, u32) = (1, 1440);

impl Default for SmartSettings {
    fn default() -> Self {
        SmartSettings {
            max_servers: 6,
            completion_timeout_ms: 5000,
            max_diagnostics: 1000,
            max_message_mb: 64,
            max_file_mb: 5,
            auto_complete: true,
            trace: false,
            idle_minutes: 10,
            typing_pause_ms: 500,
            crash_max: 3,
            crash_window_minutes: 5,
            own_args: BTreeMap::new(),
            init_options: BTreeMap::new(),
            own_paths: BTreeMap::new(),
            disabled: BTreeMap::new(),
        }
    }
}

impl SmartSettings {
    pub fn from_value(v: Option<&Value>) -> SmartSettings {
        let d = SmartSettings::default();
        let Some(o) = v.and_then(Value::as_object) else { return d };
        let num = |k: &str, def: u32, (lo, hi): (u32, u32)| o.get(k).and_then(Value::as_u64).map(|n| n.clamp(lo as u64, hi as u64) as u32).unwrap_or(def);
        SmartSettings {
            max_servers: num("max_servers", d.max_servers, MAX_SERVERS_RANGE),
            completion_timeout_ms: num("completion_timeout_ms", d.completion_timeout_ms, COMPLETION_TIMEOUT_RANGE),
            max_diagnostics: num("max_diagnostics", d.max_diagnostics, MAX_DIAGNOSTICS_RANGE),
            max_message_mb: num("max_message_mb", d.max_message_mb, MAX_MESSAGE_MB_RANGE),
            max_file_mb: num("max_file_mb", d.max_file_mb, MAX_FILE_MB_RANGE),
            auto_complete: o.get("auto_complete").and_then(Value::as_bool) != Some(false),
            trace: o.get("trace").and_then(Value::as_bool) == Some(true),
            idle_minutes: num("idle_minutes", d.idle_minutes, IDLE_MINUTES_RANGE),
            typing_pause_ms: num("typing_pause_ms", d.typing_pause_ms, TYPING_PAUSE_RANGE),
            crash_max: num("crash_max", d.crash_max, CRASH_MAX_RANGE),
            crash_window_minutes: num("crash_window_minutes", d.crash_window_minutes, CRASH_WINDOW_RANGE),
            own_args: o
                .get("own_args")
                .and_then(Value::as_object)
                .map(|m| m.iter().filter_map(|(k, v)| Some((k.chars().take(100).collect::<String>(), v.as_array()?.iter().filter_map(Value::as_str).map(|s| s.chars().take(1000).collect()).take(50).collect()))).take(200).collect())
                .unwrap_or_default(),
            init_options: o.get("init_options").and_then(Value::as_object).map(|m| m.iter().filter(|(_, v)| v.to_string().len() <= 64 * 1024).map(|(k, v)| (k.chars().take(100).collect::<String>(), v.clone())).take(200).collect()).unwrap_or_default(),
            own_paths: o
                .get("own_paths")
                .and_then(Value::as_object)
                .map(|m| m.iter().filter_map(|(k, v)| Some((k.chars().take(100).collect::<String>(), PathBuf::from(v.as_str()?)))).filter(|(_, p)| p.is_absolute()).take(200).collect())
                .unwrap_or_default(),
            disabled: o
                .get("disabled")
                .and_then(Value::as_object)
                .map(|m| m.iter().filter_map(|(k, v)| Some((k.chars().take(100).collect::<String>(), v.as_array()?.iter().filter_map(Value::as_str).map(str::to_string).take(50).collect()))).take(200).collect())
                .unwrap_or_default(),
        }
    }

    pub fn to_value(&self) -> Value {
        json!({
            "max_servers": self.max_servers,
            "completion_timeout_ms": self.completion_timeout_ms,
            "max_diagnostics": self.max_diagnostics,
            "max_message_mb": self.max_message_mb,
            "max_file_mb": self.max_file_mb,
            "auto_complete": self.auto_complete,
            "trace": self.trace,
            "idle_minutes": self.idle_minutes,
            "typing_pause_ms": self.typing_pause_ms,
            "crash_max": self.crash_max,
            "crash_window_minutes": self.crash_window_minutes,
            "own_args": self.own_args.iter().map(|(k, v)| (k.clone(), json!(v))).collect::<serde_json::Map<_, _>>(),
            "init_options": self.init_options.iter().map(|(k, v)| (k.clone(), v.clone())).collect::<serde_json::Map<_, _>>(),
            "own_paths": self.own_paths.iter().map(|(k, v)| (k.clone(), Value::String(v.to_string_lossy().to_string()))).collect::<serde_json::Map<_, _>>(),
            "disabled": self.disabled.iter().map(|(k, v)| (k.clone(), json!(v))).collect::<serde_json::Map<_, _>>(),
        })
    }

    pub fn client_limits(&self) -> client::ClientLimits {
        client::ClientLimits {
            max_message: self.max_message_mb as usize * 1024 * 1024,
            completion_timeout: Duration::from_millis(self.completion_timeout_ms as u64),
            max_diagnostics: self.max_diagnostics as usize,
            trace: self.trace,
            init_timeout: client::INIT_TIMEOUT,
        }
    }

    pub fn max_file_bytes(&self) -> u64 {
        self.max_file_mb as u64 * 1024 * 1024
    }

    pub fn typing_pause(&self) -> Duration {
        Duration::from_millis(self.typing_pause_ms as u64)
    }

    pub fn idle(&self) -> Duration {
        Duration::from_secs(self.idle_minutes as u64 * 60)
    }

    pub fn crash_window(&self) -> Duration {
        Duration::from_secs(self.crash_window_minutes as u64 * 60)
    }
}

impl serde::Serialize for SmartSettings {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.to_value().serialize(s)
    }
}

impl<'de> serde::Deserialize<'de> for SmartSettings {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<SmartSettings, D::Error> {
        let v = Value::deserialize(d)?;
        Ok(SmartSettings::from_value(Some(&v)))
    }
}


/// Schlüssel eines Servers: Sprache des Servers (Kennung) und Projektwurzel.
pub type Key = (String, PathBuf);

/// Ein Eintrag der Verwaltung.
pub struct Entry {
    pub server: String,
    pub language: String,
    pub root: PathBuf,
    pub launch: servers::Launch,
    pub client: Option<client::Client>,
    /// Vom Nutzer angehalten bzw. Start gescheitert (Grund).
    pub stopped: Option<String>,
    /// Absturz schon gezählt (der beendete Client wartet auf den Neustart).
    counted: bool,
}

impl Entry {
    pub fn status(&self) -> String {
        match (&self.stopped, &self.client) {
            (Some(why), _) => why.clone(),
            (None, Some(c)) => match &c.state {
                client::State::Starting => "starting".into(),
                client::State::Ready => c.progress.clone().map(|p| format!("ready · {p}")).unwrap_or_else(|| "ready".into()),
                client::State::Exited(w) => format!("exited ({w})"),
            },
            (None, None) => "not running".into(),
        }
    }
}

/// Alle Language Server.
#[derive(Default)]
pub struct Servers {
    pub entries: BTreeMap<Key, Entry>,
    /// Abstürze je Sprache (Zeitpunkte, für die Absturz-Regel).
    crashes: BTreeMap<String, Vec<Instant>>,
    /// Sprachen, für die der Smart-Modus in dieser Sitzung aus ist (zu oft abgestürzt).
    pub off: std::collections::BTreeSet<String>,
}

/// Ergebnis von [`Servers::ensure`].
#[derive(Debug, PartialEq)]
pub enum Ensure {
    Running(Key),
    /// Für die Sprache gibt es keinen Server (weder im PATH noch eigener Pfad).
    NoServer,
    /// Angehalten bzw. Start gescheitert.
    Stopped(String),
    /// Zu oft abgestürzt: für die Sprache in dieser Sitzung aus (Hinweis mit *Erneut versuchen*).
    CrashedOff,
    /// Start gescheitert (Fehlertext).
    Failed(String),
}

impl Servers {
    /// Server für eine Datei sicherstellen (startet ihn bei Bedarf). `resolve` sucht den Server (Tests ersetzbar).
    pub fn ensure(&mut self, lang: &str, root: &Path, settings: &SmartSettings, resolve: &dyn Fn(&str) -> Option<(String, servers::Launch)>) -> Ensure {
        if self.off.contains(lang) {
            return Ensure::CrashedOff;
        }
        let Some((server, launch)) = resolve(lang) else { return Ensure::NoServer };
        let key: Key = (server.clone(), root.to_path_buf());
        let now = Instant::now();
        if let Some(e) = self.entries.get_mut(&key) {
            if let Some(why) = &e.stopped {
                return Ensure::Stopped(why.clone());
            }
            match e.client.as_ref().map(|c| &c.state) {
                Some(client::State::Exited(_)) => {
                    // Abgestürzt: zählen; nach `crash_max` Abstürzen im Zeitfenster ist die Sprache aus.
                    if !e.counted {
                        e.counted = true;
                        let list = self.crashes.entry(lang.to_string()).or_default();
                        list.retain(|t| now.duration_since(*t) < settings.crash_window());
                        list.push(now);
                        if list.len() as u32 >= settings.crash_max {
                            self.off.insert(lang.to_string());
                            return Ensure::CrashedOff;
                        }
                    }
                }
                Some(_) => return Ensure::Running(key),
                None => {}
            }
        }
        self.make_room(settings.max_servers as usize, &key);
        let e = self.entries.entry(key.clone()).or_insert_with(|| Entry { server, language: lang.to_string(), root: root.to_path_buf(), launch: launch.clone(), client: None, stopped: None, counted: false });
        e.launch = launch;
        e.counted = false;
        if let Some(why) = e.launch.invalid.clone() {
            e.stopped = Some(why.clone());
            return Ensure::Failed(why);
        }
        let lsp_root = e.launch.lsp_root.clone().map(PathBuf::from).unwrap_or_else(|| root.to_path_buf());
        let cwd = if e.launch.lsp_root.is_some() { std::env::temp_dir() } else { root.to_path_buf() };
        match client::Client::spawn(&e.launch.program, &e.launch.args, e.launch.raw_tail.as_deref(), &cwd, &lsp_root, settings.client_limits(), settings.init_options.get(lang).cloned()) {
            Ok(c) => {
                e.client = Some(c);
                Ensure::Running(key)
            }
            Err(err) => {
                let msg = if e.launch.rustup_proxy { format!("{err} (rustup: rustup component add rust-analyzer)") } else { err };
                e.stopped = Some(msg.clone());
                Ensure::Failed(msg)
            }
        }
    }

    /// Höchstzahl einhalten: der am längsten unbenutzte laufende Server (außer `keep`) endet.
    fn make_room(&mut self, max: usize, keep: &Key) {
        loop {
            let running: Vec<(&Key, Instant)> = self.entries.iter().filter(|(k, e)| *k != keep && e.client.as_ref().is_some_and(|c| !matches!(c.state, client::State::Exited(_)))).map(|(k, e)| (k, e.client.as_ref().map(|c| c.last_used).unwrap_or_else(Instant::now))).collect();
            if running.len() < max {
                return;
            }
            let Some(oldest) = running.iter().min_by_key(|(_, t)| *t).map(|(k, _)| (*k).clone()) else { return };
            if let Some(e) = self.entries.get_mut(&oldest) {
                if let Some(mut c) = e.client.take() {
                    c.stop();
                }
                e.stopped = None;
            }
        }
    }

    pub fn client(&mut self, key: &Key) -> Option<&mut client::Client> {
        self.entries.get_mut(key).and_then(|e| e.client.as_mut())
    }

    /// Je Bild alle Server abfragen; liefert, ob sich etwas getan hat.
    pub fn tick(&mut self) -> bool {
        let mut any = false;
        for e in self.entries.values_mut() {
            if let Some(c) = e.client.as_mut() {
                any |= c.poll();
            }
        }
        any
    }

    /// Server anhalten (bleibt aus, bis neu gestartet wird).
    pub fn stop(&mut self, key: &Key) {
        if let Some(e) = self.entries.get_mut(key) {
            if let Some(mut c) = e.client.take() {
                c.stop();
            }
            e.stopped = Some(crate::state::t("angehalten", "stopped").to_string());
        }
    }

    /// Neu starten (setzt die Absturz-Zählung der Sprache zurück); der nächste [`Servers::ensure`] startet ihn.
    pub fn restart(&mut self, key: &Key) {
        if let Some(e) = self.entries.get_mut(key) {
            if let Some(mut c) = e.client.take() {
                c.stop();
            }
            e.stopped = None;
            let lang = e.language.clone();
            self.retry(&lang);
        }
    }

    /// *Erneut versuchen* nach der Absturz-Regel: Sprache wieder an, Zählung zurück.
    pub fn retry(&mut self, lang: &str) {
        self.off.remove(lang);
        self.crashes.remove(lang);
    }

    /// Server ohne offenes Dokument enden nach `idle` (Grenze aus dem Issue: 10 min).
    pub fn end_idle(&mut self, idle: Duration) {
        let now = Instant::now();
        let idle_keys: Vec<Key> = self.entries.iter().filter(|(_, e)| e.client.as_ref().is_some_and(|c| c.open_docs() == 0 && !matches!(c.state, client::State::Exited(_)) && now.duration_since(c.last_used) >= idle)).map(|(k, _)| k.clone()).collect();
        for k in idle_keys {
            if let Some(mut e) = self.entries.remove(&k)
                && let Some(mut c) = e.client.take()
            {
                c.stop();
            }
        }
    }

    /// Server einer Sprache (für *Protokoll zeigen*).
    pub fn key_of_language(&self, lang: &str) -> Option<Key> {
        self.entries.iter().find(|(_, e)| e.language == lang).map(|(k, _)| k.clone())
    }

    /// Alle Server für Dateien unter `folder` beenden (Vertrauen entzogen).
    pub fn stop_under(&mut self, folder: &Path, trust: &trust::Trust) {
        let keys: Vec<Key> = self.entries.keys().filter(|(_, root)| !trust.trusted(root)).cloned().collect();
        let _ = folder;
        for k in keys {
            if let Some(mut e) = self.entries.remove(&k)
                && let Some(mut c) = e.client.take()
            {
                c.stop();
            }
        }
    }

    pub fn running(&self) -> usize {
        self.entries.values().filter(|e| e.client.as_ref().is_some_and(|c| !matches!(c.state, client::State::Exited(_)))).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_lenient_clamped_and_round_trip() {
        let d = SmartSettings::from_value(None);
        assert_eq!(d, SmartSettings::default());
        assert_eq!((d.max_servers, d.completion_timeout_ms, d.max_diagnostics, d.max_message_mb, d.max_file_mb, d.auto_complete, d.trace), (6, 5000, 1000, 64, 5, true, false));
        let s = SmartSettings::from_value(Some(&json!({"max_servers": 999, "completion_timeout_ms": "x", "max_file_mb": 0, "auto_complete": false, "trace": 1, "own_paths": {"powershell": "C:\\tools\\pses.exe", "bad": "relative"}, "disabled": {"python": ["pylsp", 3]}})));
        assert_eq!(s.max_servers, MAX_SERVERS_RANGE.1);
        assert_eq!(s.completion_timeout_ms, 5000);
        assert_eq!(s.max_file_mb, 1);
        assert!(!s.auto_complete && !s.trace);
        if cfg!(windows) {
            assert_eq!(s.own_paths.len(), 1);
        }
        assert_eq!(s.disabled["python"], vec!["pylsp".to_string()]);
        assert_eq!(SmartSettings::from_value(Some(&s.to_value())), s);
        // Grenzen aus dem Issue, alle einstellbar
        assert_eq!((d.idle_minutes, d.typing_pause_ms, d.crash_max, d.crash_window_minutes), (10, 500, 3, 5));
        let t = SmartSettings::from_value(Some(&json!({"idle_minutes": 0, "typing_pause_ms": 99999, "crash_max": 2, "crash_window_minutes": 7, "own_args": {"c": ["--log=error"]}, "init_options": {"rust": {"check": {"command": "clippy"}}}})));
        assert_eq!((t.idle_minutes, t.typing_pause_ms, t.crash_max, t.crash_window_minutes), (1, TYPING_PAUSE_RANGE.1, 2, 7));
        assert_eq!(t.own_args["c"], vec!["--log=error".to_string()]);
        assert_eq!(SmartSettings::from_value(Some(&t.to_value())), t);
    }

    #[test]
    fn missing_servers_and_failed_starts_are_reported() {
        let mut sv = Servers::default();
        let set = SmartSettings::default();
        assert_eq!(sv.ensure("rust", Path::new("C:/p"), &set, &|_| None), Ensure::NoServer);
        let bad = servers::Launch { program: PathBuf::from(r"C:\does\not\exist\srv.exe"), args: vec![], found: PathBuf::from("x"), rustup_proxy: true, lsp_root: None, raw_tail: None, invalid: None };
        let r = sv.ensure("rust", &std::env::temp_dir(), &set, &|_| Some(("rust-analyzer".into(), bad.clone())));
        assert!(matches!(&r, Ensure::Failed(m) if m.contains("rustup component add")), "{r:?}");
        // danach angehalten, kein Dauerversuch
        assert!(matches!(sv.ensure("rust", &std::env::temp_dir(), &set, &|_| Some(("rust-analyzer".into(), bad.clone()))), Ensure::Stopped(_)));
        assert_eq!(sv.running(), 0);
    }

    /// Absturz-Regel (#367): nach `crash_max` Abstürzen im Zeitfenster ist die Sprache aus, *Erneut versuchen* hebt es
    /// auf; Leerlauf beendet Server ohne offenes Dokument.
    #[cfg(windows)]
    #[test]
    fn three_crashes_turn_the_language_off_and_retry_and_idle_work() {
        let cmd = servers::Launch { program: PathBuf::from(std::env::var("ComSpec").unwrap_or_else(|_| r"C:\Windows\System32\cmd.exe".into())), args: vec!["/c".into(), "exit".into(), "1".into()], found: PathBuf::from("cmd"), rustup_proxy: false, lsp_root: None, raw_tail: None, invalid: None };
        let mut sv = Servers::default();
        let set = SmartSettings::default();
        let dir = std::env::temp_dir();
        let res = |_: &str| Some(("crashy".to_string(), cmd.clone()));
        let mut off = false;
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(20) {
            match sv.ensure("python", &dir, &set, &res) {
                Ensure::CrashedOff => {
                    off = true;
                    break;
                }
                Ensure::Running(_) => {}
                r => panic!("{r:?}"),
            }
            sv.tick();
            std::thread::sleep(Duration::from_millis(30));
        }
        assert!(off, "nach drei Abstürzen aus");
        assert!(sv.off.contains("python"));
        assert_eq!(sv.ensure("python", &dir, &set, &res), Ensure::CrashedOff, "bleibt aus");
        assert!(matches!(sv.ensure("rust", &dir, &set, &|_| None), Ensure::NoServer), "andere Sprachen nicht betroffen");
        sv.retry("python");
        assert!(matches!(sv.ensure("python", &dir, &set, &res), Ensure::Running(_)), "Erneut versuchen");
        // Leerlauf: ohne offenes Dokument nach der Zeit beendet
        sv.end_idle(Duration::ZERO);
        assert!(sv.entries.is_empty());
    }

    #[test]
    fn wsl_servers_start_inside_the_distribution_without_a_shell() {
        let a = crate::wsl::server_args("Ubuntu", "/home/u/proj", "/home/u/.cargo/bin/rust-analyzer", &[]).unwrap();
        assert_eq!(a, ["-d", "Ubuntu", "--cd", "/home/u/proj", "--exec", "/home/u/.cargo/bin/rust-analyzer"]);
        assert!(crate::wsl::server_args("Ubuntu", "relative", "/x", &[]).is_none());
        assert!(crate::wsl::server_args("-x", "/p", "/x", &[]).is_none() && crate::wsl::server_args("a/b", "/p", "/x", &[]).is_none(), "keine Option, kein Pfad als Distribution");
        assert!(crate::wsl::server_args("Ubuntu", "/p", "x", &[]).is_none(), "Programm absolut");
        let w = crate::wsl::which_args("Ubuntu", "gopls");
        assert_eq!(w.last().map(String::as_str), Some("gopls"), "Name als eigenes Argument");
        assert!(!w.iter().any(|x| x.contains("gopls") && x.contains("command")), "nie in der Shell-Zeichenkette");
        assert!(!crate::wsl::valid_program_name("-rf") && !crate::wsl::valid_program_name("a;b") && crate::wsl::valid_program_name("vscode-json-language-server"));
        assert_eq!(crate::wsl::find_program("Ubuntu", "gopls"), None, "in Tests nie ein echtes wsl.exe");
    }
}
