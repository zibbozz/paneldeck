//! Server und Modelle in den Einstellungen (`settings.json` → `llm_servers`, öffentliches Datenformat ab 2.2.0).
//!
//! ```json
//! "llm_servers": [
//!   { "id": "srv-18a3f0c2b1d4e5f6", "name": "Ollama", "kind": "ollama", "url": "http://localhost:11434",
//!     "receive_timeout_secs": 600,
//!     "models": [ { "name": "llama3.1:8b", "level": "Read", "disabled_tools": ["read_terminal"], "ask_write": true,
//!                   "max_steps": 10, "tools": true, "context_length": 16384, "temperature": 0.7 } ],
//!     "last_models": [ { "name": "llama3.1:8b", "tools": true, "context_length": 131072 } ] }
//! ]
//! ```
//!
//! `api_key` (nur wenn gesetzt) ist das Base64 eines DPAPI-Blobs (`secret::protect`), nie Klartext. Lesen ist tolerant:
//! ein Server-Eintrag, der sich nicht lesen lässt (unbekannter `kind` einer neueren Version, falscher Typ …), wird
//! übersprungen (Hinweis im Reiter „Modelle“, beim nächsten Speichern fällt er weg); fehlende Felder haben Standardwerte.

use super::{Endpoint, ModelInfo, ServerKind};
use crate::mcp::McpLevel;
use crate::state::Lang;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::Duration;

/// Kontextlänge, wenn weder eine eigene Angabe noch eine vom Server bekannt ist.
pub const DEFAULT_CONTEXT: u32 = 8192;
pub const MIN_CONTEXT: u32 = 256;
pub const MAX_CONTEXT: u32 = 16 << 20;
pub const DEFAULT_MAX_STEPS: u32 = 10;
pub const MAX_STEPS: u32 = 100;
/// Höchstens so viele Modelle je Server werden aus der letzten Verbindung gemerkt.
pub const MAX_LAST_MODELS: usize = 500;
/// Vorschlag, wenn noch kein Server eingetragen ist.
pub const OLLAMA_LOCAL: &str = "http://localhost:11434";

/// Ein LLM-Server.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmServer {
    /// Feste Kennung (in `ChatMeta::server` der Chats), wird beim Anlegen erzeugt.
    pub id: String,
    pub name: String,
    pub kind: ServerKind,
    pub url: String,
    /// DPAPI-Blob als Base64 (`secret::protect`); leer = kein Schlüssel.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    /// Wartezeit auf Daten in Sekunden (fehlt = Standard 300 s).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receive_timeout_secs: Option<u32>,
    /// Einstellungen je Modell (nur für Modelle, die der Nutzer eingestellt hat).
    #[serde(deserialize_with = "lenient_vec")]
    pub models: Vec<LlmModel>,
    /// Ergebnis der letzten Verbindung („Verbinden“).
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "lenient_vec")]
    pub last_models: Vec<ModelInfo>,
}

impl Default for LlmServer {
    fn default() -> Self {
        LlmServer { id: String::new(), name: String::new(), kind: ServerKind::Ollama, url: String::new(), api_key: String::new(), receive_timeout_secs: None, models: Vec::new(), last_models: Vec::new() }
    }
}

/// Einstellungen eines Modells für Chats (unabhängig von der MCP-Stufe für externe Agenten).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmModel {
    pub name: String,
    /// Welche Paneldeck-Werkzeuge das Modell nutzen darf (Aus / Lesen / Steuern). Standard: Lesen.
    pub level: McpLevel,
    /// Einzeln gesperrte Werkzeuge.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub disabled_tools: Vec<String>,
    /// Vor Werkzeugen mit Schreibzugriff nachfragen (Standard: an).
    pub ask_write: bool,
    /// Höchstens so viele Werkzeugaufrufe je Antwort (1–100, Standard 10).
    pub max_steps: u32,
    /// Eigene Angabe, ob das Modell Werkzeuge aufrufen kann (fehlt = vom Server erkannt).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<bool>,
    /// Eigene Kontextlänge in Tokens (fehlt = vom Server erkannt, sonst 8192). Bei Ollama als `num_ctx` gesendet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
}

impl Default for LlmModel {
    fn default() -> Self {
        LlmModel { name: String::new(), level: McpLevel::Read, disabled_tools: Vec::new(), ask_write: true, max_steps: DEFAULT_MAX_STEPS, tools: None, context_length: None, temperature: None }
    }
}

impl LlmModel {
    pub fn named(name: &str) -> Self {
        LlmModel { name: name.to_string(), ..LlmModel::default() }
    }

    fn sanitize(&mut self) {
        self.name = self.name.trim().to_string();
        self.max_steps = self.max_steps.clamp(1, MAX_STEPS);
        self.context_length = self.context_length.filter(|c| *c > 0).map(|c| c.clamp(MIN_CONTEXT, MAX_CONTEXT));
        self.temperature = self.temperature.filter(|t| t.is_finite()).map(|t| t.clamp(0.0, 2.0));
        let mut seen = std::collections::HashSet::new();
        self.disabled_tools.retain(|t| !t.trim().is_empty() && seen.insert(t.clone()));
    }
}

/// Einträge, die beim letzten Lesen übersprungen wurden (Kurzbeschreibung), für den Hinweis im Reiter „Modelle“.
static SKIPPED: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn skipped_entries() -> Vec<String> {
    SKIPPED.lock().map(|v| v.clone()).unwrap_or_default()
}

/// Liste tolerant lesen: ungültige Einträge fallen weg, statt die ganze Datei unlesbar zu machen.
fn lenient_vec<'de, D: serde::Deserializer<'de>, T: DeserializeOwned>(d: D) -> Result<Vec<T>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(v.as_array().map(|a| a.iter().filter_map(|x| serde_json::from_value(x.clone()).ok()).collect()).unwrap_or_default())
}

/// Für `Settings::llm_servers`: wie `lenient_vec`, merkt sich übersprungene Server (z. B. unbekannter `kind`).
pub fn de_servers<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<LlmServer>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    for x in v.as_array().map(|a| a.as_slice()).unwrap_or_default() {
        match serde_json::from_value::<LlmServer>(x.clone()) {
            Ok(s) => out.push(s),
            Err(e) => {
                let name = x.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                let kind = x.get("kind").map(|k| k.to_string()).unwrap_or_default();
                skipped.push(format!("{name} (kind {kind}): {e}"));
            }
        }
    }
    if let Ok(mut s) = SKIPPED.lock() {
        *s = skipped;
    }
    Ok(out)
}

/// Neue, eindeutige Server-Kennung.
pub fn new_server_id(existing: &[LlmServer]) -> String {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    loop {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let id = format!("srv-{:016x}", t ^ n.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        if !existing.iter().any(|s| s.id == id) {
            return id;
        }
    }
}

/// Werte begrenzen, doppelte/fehlende Kennungen ersetzen, doppelte Modell-Einträge entfernen.
pub fn sanitize(servers: &mut Vec<LlmServer>) {
    let mut ids = std::collections::HashSet::new();
    for i in 0..servers.len() {
        let id = servers[i].id.trim().to_string();
        let id = if id.is_empty() || ids.contains(&id) { new_server_id(servers) } else { id };
        ids.insert(id.clone());
        let s = &mut servers[i];
        s.id = id;
        s.name = s.name.trim().to_string();
        s.url = s.url.trim().to_string();
        s.api_key = s.api_key.trim().to_string();
        s.receive_timeout_secs = s.receive_timeout_secs.filter(|t| *t > 0).map(|t| t.clamp(5, 3600));
        let mut names = std::collections::HashSet::new();
        for m in &mut s.models {
            m.sanitize();
        }
        s.models.retain(|m| !m.name.is_empty() && names.insert(m.name.clone()));
        let mut names = std::collections::HashSet::new();
        s.last_models.retain(|m| !m.name.trim().is_empty() && names.insert(m.name.clone()));
        s.last_models.truncate(MAX_LAST_MODELS);
    }
}

impl LlmServer {
    /// Anzeigename (Name, sonst Adresse).
    pub fn label(&self) -> &str {
        if self.name.is_empty() { &self.url } else { &self.name }
    }

    /// Einstellungen eines Modells (Standardwerte, wenn nichts eingestellt ist).
    pub fn model_cfg(&self, name: &str) -> LlmModel {
        self.models.iter().find(|m| m.name == name).cloned().unwrap_or_else(|| LlmModel::named(name))
    }

    /// Einstellungen eines Modells setzen; Standardwerte werden nicht gespeichert (der Eintrag fällt dann weg).
    pub fn set_model_cfg(&mut self, cfg: LlmModel) {
        let default = cfg == LlmModel::named(&cfg.name);
        match self.models.iter().position(|m| m.name == cfg.name) {
            Some(i) if default => {
                self.models.remove(i);
            }
            Some(i) => self.models[i] = cfg,
            None if default => {}
            None => self.models.push(cfg),
        }
    }

    /// Was der Server zuletzt über das Modell gemeldet hat.
    pub fn info(&self, name: &str) -> Option<&ModelInfo> {
        self.last_models.iter().find(|m| m.name == name)
    }

    /// Alle bekannten Modellnamen: zuletzt gemeldete, dann eingestellte, die der Server nicht (mehr) gemeldet hat.
    pub fn model_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.last_models.iter().map(|m| m.name.clone()).collect();
        for m in &self.models {
            if !v.contains(&m.name) {
                v.push(m.name.clone());
            }
        }
        v
    }

    /// Kann das Modell Werkzeuge aufrufen? Eigene Angabe vor Erkanntem; `None` = unbekannt.
    pub fn tools_supported(&self, name: &str) -> Option<bool> {
        self.model_cfg(name).tools.or_else(|| self.info(name).and_then(|i| i.tools))
    }

    /// Kontextlänge: eigene Angabe > vom Server erkannt > `DEFAULT_CONTEXT`.
    pub fn context_length(&self, name: &str) -> u32 {
        self.model_cfg(name).context_length.or_else(|| self.info(name).and_then(|i| i.context_length)).filter(|c| *c > 0).unwrap_or(DEFAULT_CONTEXT)
    }

    /// Verbindungsangaben (entschlüsselt den Schlüssel). Fehlertext in `lang`.
    pub fn endpoint(&self, lang: Lang) -> Result<Endpoint, String> {
        let key = if self.api_key.is_empty() {
            None
        } else {
            match super::secret::unprotect(&self.api_key) {
                Ok(k) => Some(k),
                Err(e) => {
                    return Err(format!(
                        "{} ({})",
                        lang.t(
                            "Der API-Schlüssel ließ sich nicht entschlüsseln – bitte in Einstellungen → Modelle neu eingeben.",
                            "API key could not be decrypted – enter it again in Settings → Models.",
                        ),
                        e.text(lang)
                    ))
                }
            }
        };
        let mut ep = Endpoint::new(self.kind, &self.url).with_key(key);
        if let Some(t) = self.receive_timeout_secs {
            ep = ep.with_receive_timeout(Duration::from_secs(t as u64));
        }
        Ok(ep)
    }
}

/// Schnelle Prüfung einer eingegebenen Adresse beim Tippen (ohne Namensauflösung, damit die Oberfläche nicht wartet):
/// Form, Schema und – bei Zahlenadressen – das erlaubte Netz. Servernamen prüft die Verbindung (vor jedem Aufbau).
pub fn quick_url_check(url: &str) -> Result<(), super::LlmError> {
    let u = super::net::parse(url)?;
    super::net::check_with(&u, &|_, _| Ok(vec![std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)]))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_roundtrip_and_defaults() {
        let s: LlmServer = serde_json::from_str(r#"{"id":"a","name":"N","kind":"openai","url":"http://10.0.0.2:1234/v1"}"#).unwrap();
        assert_eq!(s.kind, ServerKind::OpenAi);
        assert!(s.models.is_empty() && s.api_key.is_empty() && s.receive_timeout_secs.is_none());
        let j = serde_json::to_value(&s).unwrap();
        assert!(j.get("api_key").is_none() && j.get("last_models").is_none() && j.get("receive_timeout_secs").is_none(), "{j}");
        let m: LlmModel = serde_json::from_str(r#"{"name":"x"}"#).unwrap();
        assert_eq!((m.level, m.ask_write, m.max_steps, m.tools, m.context_length), (McpLevel::Read, true, 10, None, None));
        let m: LlmModel = serde_json::from_str(r#"{"name":"x","level":"Control","max_steps":500,"context_length":0,"temperature":9}"#).unwrap();
        assert_eq!(m.level, McpLevel::Control);
        let mut v = vec![LlmServer { id: "a".into(), models: vec![m, LlmModel::named("x")], ..Default::default() }];
        sanitize(&mut v);
        let m = &v[0].models;
        assert_eq!(m.len(), 1, "doppelter Modellname fällt weg");
        assert_eq!((m[0].max_steps, m[0].context_length, m[0].temperature), (100, None, Some(2.0)));
        // Falscher Typ der Stufe → Aus (sicher), nicht unlesbar.
        let m: LlmModel = serde_json::from_str(r#"{"name":"x","level":7}"#).unwrap();
        assert_eq!(m.level, McpLevel::Off);
    }

    #[test]
    fn unknown_kind_is_skipped_not_fatal() {
        #[derive(Deserialize)]
        struct W {
            #[serde(deserialize_with = "de_servers")]
            s: Vec<LlmServer>,
        }
        let w: W = serde_json::from_str(r#"{"s":[{"id":"a","kind":"ollama","url":"http://localhost:11434"},{"id":"b","name":"Future","kind":"anthropic"},5,{"id":"c","kind":"openai","last_models":[{"name":"m"},{"bad":1}],"models":[{"name":"m","max_steps":"x"}]}]}"#).unwrap();
        assert_eq!(w.s.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["a", "c"]);
        assert_eq!(w.s[1].last_models.len(), 1, "ungültiges Modell fällt weg, Server bleibt");
        assert!(w.s[1].models.is_empty());
    }

    #[test]
    fn ids_are_fixed_up() {
        let mut v = vec![LlmServer { id: "x".into(), ..Default::default() }, LlmServer { id: "x".into(), ..Default::default() }, LlmServer::default()];
        sanitize(&mut v);
        assert_eq!(v[0].id, "x");
        assert!(v[1].id.starts_with("srv-") && v[2].id.starts_with("srv-") && v[1].id != v[2].id);
        assert_ne!(new_server_id(&v), new_server_id(&v));
    }

    #[test]
    fn model_lookup_and_overrides() {
        let mut s = LlmServer { last_models: vec![ModelInfo { name: "a".into(), tools: Some(false), context_length: Some(4096) }, ModelInfo { name: "b".into(), tools: None, context_length: None }], ..Default::default() };
        assert_eq!(s.tools_supported("a"), Some(false));
        assert_eq!(s.tools_supported("b"), None);
        assert_eq!(s.context_length("a"), 4096);
        assert_eq!(s.context_length("b"), DEFAULT_CONTEXT);
        let mut c = s.model_cfg("a");
        c.tools = Some(true);
        c.context_length = Some(32768);
        s.set_model_cfg(c);
        assert_eq!((s.tools_supported("a"), s.context_length("a")), (Some(true), 32768));
        s.set_model_cfg(LlmModel::named("zz"));
        assert_eq!(s.models.len(), 1, "Standardwerte werden nicht gespeichert");
        s.set_model_cfg(LlmModel { level: McpLevel::Off, ..LlmModel::named("zz") });
        assert_eq!(s.model_names(), ["a", "b", "zz"]);
        s.set_model_cfg(LlmModel::named("zz"));
        assert_eq!(s.models.len(), 1, "zurück auf Standard entfernt den Eintrag");
    }

    #[test]
    fn endpoint_and_key_failure() {
        let s = LlmServer { kind: ServerKind::OpenAi, url: " http://localhost:1234/v1 ".into(), receive_timeout_secs: Some(1), ..Default::default() };
        let ep = s.endpoint(Lang::En).unwrap();
        assert_eq!((ep.kind, ep.base_url.as_str(), ep.api_key.clone(), ep.receive_timeout), (ServerKind::OpenAi, "http://localhost:1234/v1", None, crate::llm::MIN_RECEIVE_TIMEOUT));
        let bad = LlmServer { api_key: "not base64 !!".into(), ..s.clone() };
        let e = bad.endpoint(Lang::En).unwrap_err();
        assert!(e.starts_with("API key could not be decrypted – enter it again in Settings → Models."), "{e}");
        assert!(bad.endpoint(Lang::De).unwrap_err().contains("Einstellungen → Modelle"));
        #[cfg(windows)]
        {
            let good = LlmServer { api_key: crate::llm::secret::protect("sk-1").unwrap(), ..s };
            assert_eq!(good.endpoint(Lang::En).unwrap().api_key.as_deref(), Some("sk-1"));
        }
    }

    #[test]
    fn quick_url_check_rules() {
        assert!(quick_url_check("http://localhost:11434").is_ok());
        assert!(quick_url_check("http://192.168.1.5:8080/v1").is_ok());
        assert!(quick_url_check("http://my-box.lan:11434").is_ok(), "Namen prüft erst die Verbindung");
        assert!(matches!(quick_url_check("http://8.8.8.8"), Err(crate::llm::LlmError::Blocked { .. })));
        assert!(matches!(quick_url_check("ftp://localhost"), Err(crate::llm::LlmError::BadUrl(_))));
        assert!(matches!(quick_url_check(""), Err(crate::llm::LlmError::BadUrl(_))));
    }
}
