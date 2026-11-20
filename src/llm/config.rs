//! Server und Modelle in den Einstellungen (`settings.json` → `llm_servers`, öffentliches Datenformat ab 2.2.0).
//!
//! ```json
//! "llm_servers": [
//!   { "id": "srv-18a3f0c2b1d4e5f6", "name": "Ollama", "kind": "ollama", "url": "http://localhost:11434",
//!     "receive_timeout_secs": 600,
//!     "models": [ { "name": "llama3.1:8b", "level": "Read", "level_chosen": true, "disabled_tools": ["read_terminal"], "ask_write": true,
//!                   "max_steps": 10, "tools": true, "context_length": 16384, "temperature": 0.7 } ],
//!     "last_models": [ { "name": "llama3.1:8b", "tools": true, "context_length": 131072 } ] }
//! ]
//! ```
//!
//! `api_key` (nur wenn gesetzt) ist das Base64 eines DPAPI-Blobs (`secret::protect`), nie Klartext. Lesen ist tolerant:
//! ein Server-Eintrag, der sich nicht lesen lässt (unbekannter `kind` einer neueren Version, falscher Typ …), bleibt als
//! rohes JSON erhalten und wird unverändert zurückgeschrieben ([`LlmServers::unreadable`]; im Reiter „Modelle“ nur
//! lesbar, mit „Entfernen“). Fehlende Felder haben Standardwerte; ein Modell-Eintrag ist je Feld tolerant (siehe
//! [`LlmModel`]).

use super::{Endpoint, ModelInfo, ServerKind};
use crate::mcp::McpLevel;
use crate::state::Lang;
use serde::de::{DeserializeOwned, Error as _};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

/// Kontextlänge, wenn weder eine eigene Angabe noch eine vom Server bekannt ist.
pub const DEFAULT_CONTEXT: u32 = 8192;
pub const MIN_CONTEXT: u32 = 256;
pub const MAX_CONTEXT: u32 = 16 << 20;
/// Stufen des Kontext-Schiebereglers: Zweierpotenzen von 4096 bis 1M (jede Stufe genau das Doppelte).
pub const CONTEXT_STEPS: [u32; 9] = [4096, 8192, 16384, 32768, 65536, 131_072, 262_144, 524_288, 1 << 20];

/// Index der Regler-Stufe, die `v` logarithmisch am nächsten liegt (6000 → 8192); Werte außerhalb des Bereichs
/// landen auf der ersten bzw. letzten Stufe.
pub fn context_step_index(v: u32) -> usize {
    let v = u64::from(v);
    let mut best = 0;
    for (i, s) in CONTEXT_STEPS.iter().enumerate().skip(1) {
        // Grenze zwischen zwei Stufen ist das geometrische Mittel: v ≥ s/√2  ⇔  2·v² ≥ s².
        if 2 * v * v >= u64::from(*s) * u64::from(*s) {
            best = i;
        }
    }
    best
}

/// Nächste Regler-Stufe zu `v` (siehe [`context_step_index`]).
pub fn snap_context(v: u32) -> u32 {
    CONTEXT_STEPS[context_step_index(v)]
}

/// Kurzform einer Kontextlänge: `K` bis 512K, ab 1M `M`; `None`, wenn sie sich nicht glatt kürzen lässt.
pub fn context_short(v: u32) -> Option<String> {
    if v >= 1 << 20 && v % (1 << 20) == 0 {
        Some(format!("{}M", v >> 20))
    } else if v >= 1024 && v % 1024 == 0 {
        Some(format!("{}K", v >> 10))
    } else {
        None
    }
}
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
///
/// Lesen ist je Feld tolerant: ein unlesbares Feld bekommt seinen Standardwert, nur `level` wird dann „Aus“ (ein
/// kaputter Eintrag darf einem Modell keine Rechte geben). Ohne lesbaren `name` fällt der Eintrag weg.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LlmModel {
    pub name: String,
    /// Welche Paneldeck-Werkzeuge das Modell nutzen darf (Aus / Lesen / Steuern). Standard: Aus.
    pub level: McpLevel,
    /// Der Nutzer hat die Stufe selbst gewählt (Reiter „Modelle“ oder die einmalige Frage im ersten Chat). Fehlt es,
    /// fragt der erste Chat mit diesem Modell vor dem Senden nach. Nur geschrieben, wenn wahr.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub level_chosen: bool,
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
        LlmModel { name: String::new(), level: McpLevel::Off, level_chosen: false, disabled_tools: Vec::new(), ask_write: true, max_steps: DEFAULT_MAX_STEPS, tools: None, context_length: None, temperature: None }
    }
}

impl<'de> Deserialize<'de> for LlmModel {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        let o = v.as_object().ok_or_else(|| D::Error::custom("model entry is not an object"))?;
        let name = o.get("name").and_then(|n| n.as_str()).ok_or_else(|| D::Error::custom("model entry without name"))?;
        fn get<T: DeserializeOwned>(o: &serde_json::Map<String, Value>, k: &str, default: T) -> T {
            o.get(k).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or(default)
        }
        let def = LlmModel::named(name);
        // Fehlt die Stufe: Standard „Aus“. Ist sie da, aber unlesbar: ebenfalls „Aus“ (`McpLevel` liest Unbekanntes als Aus).
        let level = match o.get("level") {
            None => def.level,
            Some(v) => serde_json::from_value(v.clone()).unwrap_or(McpLevel::Off),
        };
        Ok(LlmModel {
            level,
            level_chosen: get(o, "level_chosen", false),
            disabled_tools: get(o, "disabled_tools", def.disabled_tools.clone()),
            ask_write: get(o, "ask_write", def.ask_write),
            max_steps: get(o, "max_steps", def.max_steps),
            tools: get(o, "tools", None),
            context_length: get(o, "context_length", None),
            temperature: get(o, "temperature", None),
            name: def.name,
        })
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

/// Liste tolerant lesen: ungültige Einträge fallen weg, statt die ganze Datei unlesbar zu machen.
fn lenient_vec<'de, D: serde::Deserializer<'de>, T: DeserializeOwned>(d: D) -> Result<Vec<T>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(v.as_array().map(|a| a.iter().filter_map(|x| serde_json::from_value(x.clone()).ok()).collect()).unwrap_or_default())
}

/// `Settings::llm_servers`: die lesbaren Server und – unverändert – die Einträge, die sich nicht lesen ließen (z. B. ein
/// unbekannter `kind` einer neueren Version). Beim Schreiben kommen beide in die Liste, die unlesbaren hinten.
/// Über `Deref` verhält es sich wie `Vec<LlmServer>` (nur die lesbaren).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LlmServers {
    pub list: Vec<LlmServer>,
    pub unreadable: Vec<Value>,
}

impl LlmServers {
    /// Weder lesbare noch unlesbare Einträge (dann fehlt das Feld in `settings.json`).
    pub fn is_blank(&self) -> bool {
        self.list.is_empty() && self.unreadable.is_empty()
    }
}

impl From<Vec<LlmServer>> for LlmServers {
    fn from(list: Vec<LlmServer>) -> Self {
        LlmServers { list, unreadable: Vec::new() }
    }
}

impl std::ops::Deref for LlmServers {
    type Target = Vec<LlmServer>;
    fn deref(&self) -> &Vec<LlmServer> {
        &self.list
    }
}

impl std::ops::DerefMut for LlmServers {
    fn deref_mut(&mut self) -> &mut Vec<LlmServer> {
        &mut self.list
    }
}

impl Serialize for LlmServers {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut seq = s.serialize_seq(Some(self.list.len() + self.unreadable.len()))?;
        for x in &self.list {
            seq.serialize_element(x)?;
        }
        for x in &self.unreadable {
            seq.serialize_element(x)?;
        }
        seq.end()
    }
}

impl<'de> Deserialize<'de> for LlmServers {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        let mut out = LlmServers::default();
        for x in v.as_array().cloned().unwrap_or_default() {
            match serde_json::from_value::<LlmServer>(x.clone()) {
                Ok(s) => out.list.push(s),
                Err(_) => out.unreadable.push(x),
            }
        }
        Ok(out)
    }
}

/// Kurzbeschreibung eines unlesbaren Eintrags (Name, Typ) und der Grund (technisch, nur für einen Tooltip).
pub fn describe_unreadable(v: &Value) -> (String, String) {
    let name = v.get("name").and_then(|n| n.as_str()).or_else(|| v.get("url").and_then(|n| n.as_str())).unwrap_or("?");
    let kind = v.get("kind").map(|k| k.as_str().map(str::to_string).unwrap_or_else(|| k.to_string())).unwrap_or_default();
    let why = serde_json::from_value::<LlmServer>(v.clone()).err().map(|e| e.to_string()).unwrap_or_default();
    (if kind.is_empty() { name.to_string() } else { format!("{name} ({kind})") }, why)
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

    /// Kontextlänge zur Anzeige: eigene Angabe > vom Server erkannt > `DEFAULT_CONTEXT`.
    pub fn context_length(&self, name: &str) -> u32 {
        self.model_cfg(name).context_length.or_else(|| self.info(name).and_then(|i| i.context_length)).filter(|c| *c > 0).unwrap_or(DEFAULT_CONTEXT)
    }

    /// Kontextlänge, mit der angefragt (und auf die der Verlauf gekürzt) wird. Eigene Angabe gilt immer. Ohne sie:
    /// bei Ollama so groß wie für Systemprompt und Werkzeuge nötig ([`auto_context`], die Zahl geht als `num_ctx` mit;
    /// mehr als nötig kostet viel Speicher), bei OpenAI-kompatiblen Servern die erkannte Länge bzw. `DEFAULT_CONTEXT`.
    /// `prompt_tokens` = geschätzte Tokens von Systemprompt und Werkzeug-Beschreibungen (`None` = unbekannt: 8192).
    pub fn request_context(&self, name: &str, prompt_tokens: Option<usize>) -> u32 {
        if let Some(c) = self.user_context(name) {
            return c;
        }
        let detected = self.info(name).and_then(|i| i.context_length).filter(|c| *c > 0);
        match self.kind {
            ServerKind::Ollama => auto_context(prompt_tokens.unwrap_or(0), detected),
            ServerKind::OpenAi => detected.unwrap_or(DEFAULT_CONTEXT),
        }
    }

    /// Eigene Kontextlänge des Nutzers (Einstellungen → Modelle), wenn gesetzt.
    pub fn user_context(&self, name: &str) -> Option<u32> {
        self.model_cfg(name).context_length.filter(|c| *c > 0)
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

/// Ohne eigene Angabe höchstens so viel Kontext (Ollama reserviert Speicher für die ganze Länge).
pub const MAX_AUTO_CONTEXT: u32 = 32768;
/// Platz für Verlauf und Werkzeug-Ergebnisse, der neben Systemprompt und Werkzeugen mindestens bleiben soll.
pub const MIN_ROOM_TOKENS: usize = 4096;
/// Anteil des Kontexts, der für die Antwort frei bleibt (Prozent; wie `chat::llm_backend::ANSWER_RESERVE_PERCENT`).
pub const ANSWER_RESERVE_PERCENT: usize = 25;

/// Kontextlänge, die Systemprompt und Werkzeuge (`prompt_tokens`) samt [`MIN_ROOM_TOKENS`] und der Antwort-Reserve
/// braucht: auf die nächste Zweierpotenz aufgerundet, mindestens `DEFAULT_CONTEXT`, höchstens [`MAX_AUTO_CONTEXT`] und
/// höchstens die erkannte Länge des Modells.
pub fn auto_context(prompt_tokens: usize, detected: Option<u32>) -> u32 {
    let need = (prompt_tokens + MIN_ROOM_TOKENS).saturating_mul(100).div_ceil(100 - ANSWER_RESERVE_PERCENT);
    let need = u32::try_from(need).unwrap_or(u32::MAX).checked_next_power_of_two().unwrap_or(MAX_AUTO_CONTEXT);
    let c = need.clamp(DEFAULT_CONTEXT, MAX_AUTO_CONTEXT);
    detected.map_or(c, |d| c.min(d))
}

/// Ist die Kontextlänge `ctx` zu klein für Systemprompt und Werkzeuge (`prompt_tokens`) samt Mindestplatz und Reserve?
pub fn context_too_small(ctx: u32, prompt_tokens: usize) -> bool {
    (ctx as usize) * (100 - ANSWER_RESERVE_PERCENT) / 100 < prompt_tokens + MIN_ROOM_TOKENS
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
    fn context_steps_double_exactly() {
        assert_eq!(CONTEXT_STEPS[0], 4096);
        assert_eq!(*CONTEXT_STEPS.last().unwrap(), 1_048_576);
        assert!(CONTEXT_STEPS.windows(2).all(|w| w[1] == w[0] * 2));
        assert!(CONTEXT_STEPS.iter().all(|s| (MIN_CONTEXT..=MAX_CONTEXT).contains(s)));
    }

    #[test]
    fn context_snaps_logarithmically() {
        for (i, s) in CONTEXT_STEPS.iter().enumerate() {
            assert_eq!(context_step_index(*s), i, "Stufe {s} bleibt");
        }
        assert_eq!(snap_context(6000), 8192, "6000 liegt logarithmisch näher an 8192");
        assert_eq!(snap_context(5700), 4096, "unter 4096·√2 ≈ 5793");
        assert_eq!(snap_context(5792), 4096);
        assert_eq!(snap_context(5793), 8192);
        assert_eq!(snap_context(12_000), 16384);
        assert_eq!(snap_context(100_000), 131_072);
        assert_eq!(snap_context(90_000), 65536);
        // Alte Werte außerhalb: erste bzw. letzte Stufe (nur für die Reglerposition).
        assert_eq!(snap_context(MIN_CONTEXT), 4096);
        assert_eq!(snap_context(1000), 4096);
        assert_eq!(snap_context(0), 4096);
        assert_eq!(snap_context(2 << 20), 1 << 20);
        assert_eq!(snap_context(MAX_CONTEXT), 1 << 20);
        assert_eq!(snap_context(u32::MAX), 1 << 20, "kein Überlauf");
    }

    #[test]
    fn context_short_labels() {
        assert_eq!(context_short(4096).as_deref(), Some("4K"));
        assert_eq!(context_short(32768).as_deref(), Some("32K"));
        assert_eq!(context_short(524_288).as_deref(), Some("512K"));
        assert_eq!(context_short(1 << 20).as_deref(), Some("1M"));
        assert_eq!(context_short(16 << 20).as_deref(), Some("16M"));
        assert_eq!(context_short(6000), None);
        assert_eq!(context_short(256), None);
    }

    #[test]
    fn server_roundtrip_and_defaults() {
        let s: LlmServer = serde_json::from_str(r#"{"id":"a","name":"N","kind":"openai","url":"http://10.0.0.2:1234/v1"}"#).unwrap();
        assert_eq!(s.kind, ServerKind::OpenAi);
        assert!(s.models.is_empty() && s.api_key.is_empty() && s.receive_timeout_secs.is_none());
        let j = serde_json::to_value(&s).unwrap();
        assert!(j.get("api_key").is_none() && j.get("last_models").is_none() && j.get("receive_timeout_secs").is_none(), "{j}");
        let m: LlmModel = serde_json::from_str(r#"{"name":"x"}"#).unwrap();
        assert_eq!((m.level, m.level_chosen, m.ask_write, m.max_steps, m.tools, m.context_length), (McpLevel::Off, false, true, 10, None, None));
        // level_chosen: nur geschrieben, wenn wahr; hin und zurück
        assert!(serde_json::to_value(&m).unwrap().get("level_chosen").is_none());
        let chosen = LlmModel { level_chosen: true, ..m.clone() };
        let j = serde_json::to_value(&chosen).unwrap();
        assert_eq!(j["level_chosen"], true);
        assert_eq!(serde_json::from_value::<LlmModel>(j).unwrap(), chosen);
        assert!(!serde_json::from_str::<LlmModel>(r#"{"name":"x","level_chosen":"yes"}"#).unwrap().level_chosen, "unlesbar → nicht gewählt");
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
    fn model_entry_is_lenient_per_field_and_never_gains_rights() {
        // Stufe Aus, ein anderes Feld kaputt: Eintrag bleibt, Stufe bleibt Aus.
        let m: LlmModel = serde_json::from_str(r#"{"name":"x","level":"Off","max_steps":"many","ask_write":false,"tools":"yes"}"#).unwrap();
        assert_eq!((m.level, m.max_steps, m.ask_write, m.tools), (McpLevel::Off, DEFAULT_MAX_STEPS, false, None));
        // Unlesbare Stufe → Aus.
        for lvl in [r#""ctrl""#, "null", "[1]", r#"{"a":1}"#] {
            let m: LlmModel = serde_json::from_str(&format!(r#"{{"name":"x","level":{lvl},"disabled_tools":5}}"#)).unwrap();
            assert_eq!(m.level, McpLevel::Off, "{lvl}");
            assert!(m.disabled_tools.is_empty());
        }
        // Fehlende Stufe → Standard Aus; ohne Namen kein Eintrag.
        assert_eq!(serde_json::from_str::<LlmModel>(r#"{"name":"x"}"#).unwrap().level, McpLevel::Off);
        assert!(serde_json::from_str::<LlmModel>(r#"{"level":"Off"}"#).is_err());
        let s: LlmServer = serde_json::from_str(r#"{"id":"a","models":[{"name":"x","level":"Off","context_length":-1}]}"#).unwrap();
        assert_eq!(s.model_cfg("x").level, McpLevel::Off);
    }

    #[test]
    fn auto_context_rounds_up_and_detects_too_small_contexts() {
        assert_eq!(auto_context(0, None), DEFAULT_CONTEXT);
        assert_eq!(auto_context(2_000, None), DEFAULT_CONTEXT, "(2000 + 4096) / 0,75 = 8128");
        assert_eq!(auto_context(2_100, None), 16384);
        assert_eq!(auto_context(20_000, None), 32768);
        assert_eq!(auto_context(20_000, Some(4096)), 4096);
        assert!(context_too_small(8192, 6_612) && !context_too_small(16384, 6_612));
        assert!(!context_too_small(8192, 2_000));
    }

    #[test]
    fn request_context_for_ollama_and_openai() {
        let mut s = LlmServer { last_models: vec![ModelInfo { name: "big".into(), tools: None, context_length: Some(131072) }, ModelInfo { name: "small".into(), tools: None, context_length: Some(2048) }], ..Default::default() };
        assert_eq!(s.request_context("big", None), DEFAULT_CONTEXT, "Ollama: ohne bekannten Bedarf 8192");
        assert_eq!(s.request_context("big", Some(9_000)), 32768, "Bedarf (9000 + 4096) / 0,75 → 32768");
        assert_eq!(s.request_context("big", Some(5_000)), 16384);
        assert_eq!(s.request_context("big", Some(500_000)), MAX_AUTO_CONTEXT, "höchstens 32768");
        assert_eq!(s.request_context("small", Some(9_000)), 2048, "höchstens die Länge des Modells");
        assert_eq!(s.request_context("small", None), 2048);
        assert_eq!(s.request_context("unknown", None), DEFAULT_CONTEXT);
        s.set_model_cfg(LlmModel { context_length: Some(32768), ..LlmModel::named("big") });
        assert_eq!(s.request_context("big", None), 32768);
        s.kind = ServerKind::OpenAi;
        assert_eq!(s.request_context("small", None), 2048);
        s.last_models[1].context_length = Some(65536);
        assert_eq!(s.request_context("small", None), 65536);
    }

    #[test]
    fn unreadable_entries_are_kept_unchanged() {
        let text = r#"[{"id":"a","kind":"ollama","url":"http://localhost:11434"},{"id":"b","name":"Future","kind":"anthropic","api_key":"BLOB","extra":{"x":[1,2]}},5,{"id":"c","kind":"openai","last_models":[{"name":"m"},{"bad":1}],"models":[{"name":"m","max_steps":"x"},{"level":"Off"}]}]"#;
        let s: LlmServers = serde_json::from_str(text).unwrap();
        assert_eq!(s.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["a", "c"]);
        assert_eq!(s[1].last_models.len(), 1, "ungültiges Modell fällt weg, Server bleibt");
        assert_eq!(s[1].models.len(), 1, "Modell mit kaputtem Feld bleibt, ohne Namen fällt es weg");
        assert_eq!(s.unreadable.len(), 2);
        assert_eq!(describe_unreadable(&s.unreadable[0]).0, "Future (anthropic)");
        // Hin und zurück: unlesbare Einträge (samt Schlüssel) unverändert.
        let back: Value = serde_json::to_value(&s).unwrap();
        let arr = back.as_array().unwrap();
        assert_eq!(arr.len(), 4);
        let orig: Value = serde_json::from_str(text).unwrap();
        assert_eq!(arr[2], orig[1]);
        assert_eq!(arr[3], orig[2]);
        let again: LlmServers = serde_json::from_value(back).unwrap();
        assert_eq!(again.unreadable, s.unreadable);
        assert!(LlmServers::default().is_blank() && !s.is_blank());
        let only_bad: LlmServers = serde_json::from_str("[5]").unwrap();
        assert!(only_bad.is_empty() && !only_bad.is_blank(), "Deref = nur lesbare");
        assert!(serde_json::from_str::<LlmServers>(r#""nope""#).unwrap().is_blank());
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
        // „Aus“ bewusst gewählt ist kein Standard mehr (sonst fragte der erste Chat erneut)
        s.set_model_cfg(LlmModel { level: McpLevel::Off, level_chosen: true, ..LlmModel::named("zz") });
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
