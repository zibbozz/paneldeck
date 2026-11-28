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

impl Default for SmartSettings {
    fn default() -> Self {
        SmartSettings { max_servers: 6, completion_timeout_ms: 5000, max_diagnostics: 1000, max_message_mb: 64, max_file_mb: 5, auto_complete: true, trace: false, own_paths: BTreeMap::new(), disabled: BTreeMap::new() }
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
        }
    }

    pub fn max_file_bytes(&self) -> u64 {
        self.max_file_mb as u64 * 1024 * 1024
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

/// Wie lange ein abgestürzter Server neu gestartet wird: höchstens so oft …
const MAX_RESTARTS: u32 = 3;
/// … innerhalb dieser Zeit; danach bleibt er aus (Meldung, *Neu starten* im Fenster „Language Server“).
const RESTART_WINDOW: Duration = Duration::from_secs(180);

/// Schlüssel eines Servers: Sprache des Servers (Kennung) und Projektwurzel.
pub type Key = (String, PathBuf);

/// Ein Eintrag der Verwaltung.
pub struct Entry {
    pub server: String,
    pub language: String,
    pub root: PathBuf,
    pub launch: servers::Launch,
    pub client: Option<client::Client>,
    /// Starts in [`RESTART_WINDOW`] (Absturz-Behandlung).
    restarts: Vec<Instant>,
    /// Aufgegeben (zu oft abgestürzt) bzw. vom Nutzer angehalten.
    pub stopped: Option<String>,
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
}

/// Ergebnis von [`Servers::ensure`].
#[derive(Debug, PartialEq)]
pub enum Ensure {
    Running(Key),
    /// Für die Sprache gibt es keinen Server (weder im PATH noch eigener Pfad).
    NoServer,
    /// Zu oft abgestürzt bzw. angehalten.
    Stopped(String),
    /// Start gescheitert (Fehlertext).
    Failed(String),
}

impl Servers {
    /// Server für eine Datei sicherstellen (startet ihn bei Bedarf). `resolve` sucht den Server (Tests ersetzbar).
    pub fn ensure(&mut self, lang: &str, root: &Path, settings: &SmartSettings, resolve: &dyn Fn(&str) -> Option<(String, servers::Launch)>) -> Ensure {
        let Some((server, launch)) = resolve(lang) else { return Ensure::NoServer };
        let key: Key = (server.clone(), root.to_path_buf());
        if let Some(e) = self.entries.get_mut(&key) {
            if let Some(why) = &e.stopped {
                return Ensure::Stopped(why.clone());
            }
            let alive = e.client.as_ref().is_some_and(|c| !matches!(c.state, client::State::Exited(_)));
            if alive {
                return Ensure::Running(key);
            }
            // Abgestürzt: neu starten, höchstens MAX_RESTARTS-mal im Zeitfenster.
            let now = Instant::now();
            e.restarts.retain(|t| now.duration_since(*t) < RESTART_WINDOW);
            if e.restarts.len() as u32 >= MAX_RESTARTS {
                let why = crate::state::t("zu oft abgestürzt – im Fenster „Language Server“ neu starten", "crashed too often – restart it in the “Language servers” window").to_string();
                e.stopped = Some(why.clone());
                e.client = None;
                return Ensure::Stopped(why);
            }
        }
        self.make_room(settings.max_servers as usize, &key);
        let e = self.entries.entry(key.clone()).or_insert_with(|| Entry { server, language: lang.to_string(), root: root.to_path_buf(), launch: launch.clone(), client: None, restarts: Vec::new(), stopped: None });
        e.launch = launch;
        e.restarts.push(Instant::now());
        match client::Client::spawn(&e.launch.program, &e.launch.args, root, root, settings.client_limits()) {
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

    /// Neu starten (setzt die Absturz-Zählung zurück); der nächste [`Servers::ensure`] startet ihn.
    pub fn restart(&mut self, key: &Key) {
        if let Some(e) = self.entries.get_mut(key) {
            if let Some(mut c) = e.client.take() {
                c.stop();
            }
            e.stopped = None;
            e.restarts.clear();
        }
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
    }

    #[test]
    fn missing_servers_and_failed_starts_are_reported() {
        let mut sv = Servers::default();
        let set = SmartSettings::default();
        assert_eq!(sv.ensure("rust", Path::new("C:/p"), &set, &|_| None), Ensure::NoServer);
        let bad = servers::Launch { program: PathBuf::from(r"C:\does\not\exist\srv.exe"), args: vec![], found: PathBuf::from("x"), rustup_proxy: true };
        let r = sv.ensure("rust", &std::env::temp_dir(), &set, &|_| Some(("rust-analyzer".into(), bad.clone())));
        assert!(matches!(&r, Ensure::Failed(m) if m.contains("rustup component add")), "{r:?}");
        // danach angehalten, kein Dauerversuch
        assert!(matches!(sv.ensure("rust", &std::env::temp_dir(), &set, &|_| Some(("rust-analyzer".into(), bad.clone()))), Ensure::Stopped(_)));
        assert_eq!(sv.running(), 0);
    }
}
