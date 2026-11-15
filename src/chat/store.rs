//! Verlaufsdatei eines Chats: `chats/<id>.json` im Datenordner (öffentliches Datenformat ab 2.2.0).
//!
//! ```json
//! {
//!   "version": 1,
//!   "context_start": 2,
//!   "always_allow": ["send_text"],
//!   "messages": [
//!     { "role": "user", "content": "Which sessions are open?", "ts": 1763200000 },
//!     { "role": "assistant", "content": "", "ts": 1763200001,
//!       "tool_calls": [ { "id": "c1", "name": "list_sessions", "arguments": {} } ] },
//!     { "role": "tool", "content": "{\"sessions\":[]}", "ts": 1763200002,
//!       "tool_call_id": "c1", "tool_name": "list_sessions", "status": "ok" },
//!     { "role": "assistant", "content": "There are no sessions.", "ts": 1763200003 },
//!     { "role": "assistant", "content": "", "ts": 1763200010, "error": "connection refused" }
//!   ]
//! }
//! ```
//!
//! Lesen ist tolerant: BOM erlaubt, unbekannte Felder werden ignoriert, eine einzelne ungültige Nachricht wird
//! übersprungen; eine unlesbare oder zu große Datei ergibt einen leeren Chat mit Hinweis (die Datei wird vorher als
//! `<id>.defekt-<zeit>.json` gesichert). Geschrieben wird atomar (temporäre Datei, dann umbenennen).

use crate::state;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Aktuelle Version des Formats.
pub const VERSION: u32 = 1;
/// Größte Datei, die geladen wird; größere Verläufe kürzt das Speichern vorher (älteste Nachrichten zuerst).
pub const MAX_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    #[default]
    User,
    Assistant,
    Tool,
}

/// Ergebnis eines Werkzeugaufrufs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolStatus {
    Ok,
    Error,
    /// Vom Nutzer abgelehnt (oder durch die Einstellungen gesperrt).
    Denied,
}

/// Werkzeugaufruf, den das Modell in einer Antwort verlangt.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Parameter als JSON (meist ein Objekt).
    pub arguments: serde_json::Value,
}

/// Eine Nachricht im Verlauf.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatMsg {
    pub role: Role,
    pub content: String,
    /// Zeitpunkt (Sekunden seit 1970, UTC).
    pub ts: u64,
    /// Nur Antworten des Modells: verlangte Werkzeugaufrufe.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// Nur Werkzeug-Nachrichten: zu welchem Aufruf das Ergebnis gehört.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<ToolStatus>,
    /// Nur Antworten des Modells: Fehler, mit dem die Antwort abbrach (Anzeige mit „Wiederholen“).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Nur Antworten des Modells: Hinweis unter der Antwort, z. B. „an der Token-Grenze abgeschnitten“ (seit 2.2.0; wird
    /// nicht an das Modell gesendet).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl ChatMsg {
    pub fn new(role: Role, content: impl Into<String>) -> Self {
        ChatMsg { role, content: content.into(), ts: super::now_secs(), ..ChatMsg::default() }
    }
}

/// Inhalt von `chats/<id>.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatFile {
    pub version: u32,
    /// Index der ersten Nachricht, die das Modell bei der letzten Anfrage noch gesehen hat (ältere wurden wegen der
    /// Kontextlänge weggelassen). Fehlt, solange nichts gekürzt wurde.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_start: Option<usize>,
    /// Schreibende Werkzeuge, die der Nutzer in diesem Chat „immer erlaubt“ hat (keine Nachfrage mehr; seit 2.2.0).
    /// Steht vor `messages`, weil `serialize_capped` die Nachrichten ans Ende setzt.
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "lenient_names")]
    pub always_allow: Vec<String>,
    pub messages: Vec<ChatMsg>,
}

impl Default for ChatFile {
    fn default() -> Self {
        ChatFile { version: VERSION, context_start: None, always_allow: Vec::new(), messages: Vec::new() }
    }
}

/// Liste von Werkzeugnamen tolerant lesen: Nicht-Texte fallen weg, ein falscher Typ ergibt eine leere Liste.
fn lenient_names<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default())
}

/// Ordner der Verlaufsdateien im Datenordner.
pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("chats")
}

/// Pfad der Verlaufsdatei eines Chats.
pub fn path(data_dir: &Path, id: u64) -> PathBuf {
    dir(data_dir).join(format!("{id}.json"))
}

/// Tolerant laden. Fehlt die Datei, ist der Chat einfach leer (ohne Hinweis); sonst gibt es bei Problemen einen
/// Hinweistext für die Oberfläche.
pub fn load(path: &Path) -> (ChatFile, Option<String>) {
    let Ok(meta) = std::fs::metadata(path) else { return (ChatFile::default(), None) };
    if meta.len() > MAX_BYTES {
        let bk = backup_note(path);
        return (ChatFile::default(), Some(format!("{} ({bk})", state::t("Chatverlauf ist zu groß und wurde nicht geladen", "Chat history is too large and was not loaded"))));
    }
    let text = match std::fs::read(path) {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(e) => return (ChatFile::default(), Some(format!("{}: {e}", state::t("Chatverlauf nicht lesbar", "Chat history could not be read")))),
    };
    parse(state::strip_bom(&text)).map_or_else(
        || {
            let bk = backup_note(path);
            (ChatFile::default(), Some(format!("{} ({bk})", state::t("Chatverlauf war beschädigt – leerer Chat", "Chat history was corrupt – empty chat"))))
        },
        |(file, skipped)| {
            let note = (skipped > 0).then(|| {
                let bk = backup_note(path);
                if state::lang() == state::Lang::De {
                    format!("{skipped} ungültige Nachricht(en) im Chatverlauf übersprungen ({bk})")
                } else {
                    format!("{skipped} invalid message(s) in the chat history skipped ({bk})")
                }
            });
            (file, note)
        },
    )
}

/// Text parsen: `None`, wenn es kein JSON-Objekt ist; sonst die Datei und die Zahl übersprungener Nachrichten.
pub fn parse(text: &str) -> Option<(ChatFile, usize)> {
    let mut v: serde_json::Value = serde_json::from_str(text).ok()?;
    let obj = v.as_object_mut()?;
    let msgs = obj.remove("messages");
    let mut file = serde_json::from_value::<ChatFile>(serde_json::Value::Object(std::mem::take(obj))).unwrap_or_default();
    let mut skipped = 0;
    match msgs {
        Some(serde_json::Value::Array(items)) => {
            for it in items {
                match serde_json::from_value::<ChatMsg>(it) {
                    Ok(m) => file.messages.push(m),
                    Err(_) => skipped += 1,
                }
            }
        }
        None | Some(serde_json::Value::Null) => {}
        Some(_) => skipped += 1,
    }
    if file.context_start.is_some_and(|c| c > file.messages.len()) {
        file.context_start = None;
    }
    Some((file, skipped))
}

fn backup_note(path: &Path) -> String {
    match state::backup_corrupt(path) {
        Some(b) => format!("{} {}", state::t("Sicherung:", "backup:"), b.display()),
        None => state::t("keine Sicherung möglich", "no backup possible").to_string(),
    }
}

/// Atomar speichern (Ordner wird angelegt). Ein Verlauf über [`MAX_BYTES`] verliert vorher seine ältesten Nachrichten
/// (Systemnachrichten am Anfang bleiben), damit er beim nächsten Start wieder geladen wird.
pub fn save(path: &Path, file: &ChatFile) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let data = serialize_capped(file, MAX_BYTES as usize);
    state::write_atomic(path, &data)
}

/// JSON der Datei (kompakt), höchstens `cap` Bytes groß: jede Nachricht wird genau einmal serialisiert; reicht der Platz
/// nicht, fallen die ältesten Nachrichten nach den Systemnachrichten am Anfang weg (anhand der gemessenen Größen).
pub fn serialize_capped(file: &ChatFile, cap: usize) -> Vec<u8> {
    let parts: Vec<Vec<u8>> = file.messages.iter().map(|m| serde_json::to_vec(m).unwrap_or_else(|_| b"{}".to_vec())).collect();
    let keep_from = file.messages.iter().take_while(|m| m.role == Role::System).count();
    let size = |from: usize| -> usize { parts[..keep_from].iter().chain(&parts[from..]).map(|p| p.len() + 1).sum() };
    // Kopf ohne Nachrichten (+ Reserve für context_start): `{"version":1,"context_start":N,"always_allow":[…],"messages":[]}`
    let head_len = 64 + serde_json::to_vec(&file.always_allow).map(|v| v.len()).unwrap_or(0) + 16;
    let mut drop_to = keep_from;
    if head_len + size(keep_from) > cap {
        let mut total = size(keep_from);
        while drop_to < parts.len() && head_len + total > cap {
            total -= parts[drop_to].len() + 1;
            drop_to += 1;
        }
    }
    let dropped = drop_to - keep_from;
    // Gekürzt: der Index rückt nach; lag er im weggefallenen Teil, sieht das Modell ohnehin alles Übrige.
    let context_start = if dropped == 0 { file.context_start } else { file.context_start.and_then(|c| (c >= drop_to).then(|| c - dropped)).filter(|c| *c > keep_from) };
    let head = ChatFile { version: file.version, context_start, always_allow: file.always_allow.clone(), messages: Vec::new() };
    let mut data = serde_json::to_vec(&head).unwrap_or_default();
    // `messages` ist das letzte Feld: `…"messages":[]}` → Nachrichten zwischen die Klammern setzen.
    data.truncate(data.len().saturating_sub(2));
    let mut first = true;
    for p in parts[..keep_from].iter().chain(&parts[drop_to..]) {
        if !first {
            data.push(b',');
        }
        first = false;
        data.extend_from_slice(p);
    }
    data.extend_from_slice(b"]}");
    data
}

/// Höchste Sitzungs-Id, zu der es im Datenordner noch Dateien gibt (`chats/<id>…`, `scrollback/<id>…`). Damit vergibt
/// der Hub nach einem Absturz oder einer zurückgesetzten `state.json` keine Id, deren alter Verlauf noch daliegt.
pub fn max_used_id(data_dir: &Path) -> u64 {
    let mut max = 0;
    for sub in ["chats", "scrollback"] {
        let Ok(rd) = std::fs::read_dir(data_dir.join(sub)) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let digits: String = name.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.len() < name.len() && name[digits.len()..].starts_with('.') {
                if let Ok(n) = digits.parse::<u64>() {
                    max = max.max(n);
                }
            }
        }
    }
    max
}

/// Fremden Verlauf unter dieser Id beiseitelegen (`<id>.orphan-<zeit>.json`), statt ihn zu laden oder zu überschreiben.
pub fn set_aside(path: &Path) -> Option<PathBuf> {
    if !path.exists() {
        return None;
    }
    let stem = path.file_stem()?.to_string_lossy().to_string();
    let secs = super::now_secs();
    let mut target = path.with_file_name(format!("{stem}.orphan-{secs}.json"));
    let mut n = 2;
    while target.exists() {
        target = path.with_file_name(format!("{stem}.orphan-{secs}-{n}.json"));
        n += 1;
    }
    std::fs::rename(path, &target).ok()?;
    Some(target)
}

/// Verlauf eines geschlossenen Chats löschen, samt seiner Sicherungen beschädigter Dateien (`<id>.defekt-*.json`).
pub fn remove(data_dir: &Path, id: u64) {
    let _ = std::fs::remove_file(path(data_dir, id));
    let prefix = format!("{id}.defekt-");
    if let Ok(rd) = std::fs::read_dir(dir(data_dir)) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with(&prefix) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pd-chat-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn sample() -> ChatFile {
        let mut a = ChatMsg::new(Role::Assistant, "");
        a.tool_calls.push(ToolCall { id: "c1".into(), name: "list_sessions".into(), arguments: serde_json::json!({}) });
        let mut t = ChatMsg::new(Role::Tool, "{\"sessions\":[]}");
        t.tool_call_id = Some("c1".into());
        t.tool_name = Some("list_sessions".into());
        t.status = Some(ToolStatus::Ok);
        let mut e = ChatMsg::new(Role::Assistant, "");
        e.error = Some("connection refused".into());
        ChatFile { version: VERSION, context_start: Some(1), always_allow: vec!["send_text".into()], messages: vec![ChatMsg::new(Role::User, "Hi ä 😀"), a, t, ChatMsg::new(Role::Assistant, "**done**"), e] }
    }

    #[test]
    fn always_allow_is_optional_lenient_and_survives_the_size_cap() {
        // Datei von 2.2.0-Vorabständen ohne das Feld
        let (f, skipped) = parse(r#"{"version":1,"messages":[{"role":"user","content":"x"}]}"#).unwrap();
        assert_eq!((f.always_allow.len(), skipped, f.messages.len()), (0, 0, 1));
        assert!(!String::from_utf8(serialize_capped(&f, 1000)).unwrap().contains("always_allow"), "leer: nicht geschrieben");
        // Falscher Typ / falsche Einträge: kein Verlust des Verlaufs
        let (f, _) = parse(r#"{"always_allow":"send_text","messages":[{"role":"user","content":"x"}]}"#).unwrap();
        assert!(f.always_allow.is_empty() && f.messages.len() == 1);
        let (f, _) = parse(r#"{"always_allow":["send_text",5,"run_command"],"messages":[]}"#).unwrap();
        assert_eq!(f.always_allow, ["send_text", "run_command"]);
        // Gekürzt gespeichert: das Feld bleibt
        let mut big = sample();
        for i in 0..100 {
            big.messages.push(ChatMsg::new(Role::User, format!("{i}{}", "z".repeat(200))));
        }
        let data = serialize_capped(&big, 4_000);
        assert!(data.len() <= 4_000);
        let (back, _) = parse(std::str::from_utf8(&data).unwrap()).unwrap();
        assert_eq!(back.always_allow, ["send_text"]);
    }

    #[test]
    fn roundtrip_atomic_without_temp_file() {
        let d = tmp("rt");
        let p = path(&d, 5);
        let f = sample();
        save(&p, &f).unwrap();
        assert!(!dir(&d).join("5.json.tmp").exists());
        let (back, note) = load(&p);
        assert!(note.is_none(), "{note:?}");
        assert_eq!(back, f);
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("\"version\":1"));
        assert!(text.contains("\"status\":\"ok\""));
        // Leere optionale Felder werden nicht geschrieben
        assert!(!text.contains("\"tool_calls\":[]"));
        // Ohne context_start und ohne Nachrichten bleibt es gültiges JSON
        let (empty, _) = parse(std::str::from_utf8(&serialize_capped(&ChatFile::default(), 1000)).unwrap()).unwrap();
        assert_eq!(empty, ChatFile::default());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn missing_file_is_an_empty_chat_without_warning() {
        let (f, note) = load(&tmp("none").join("1.json"));
        assert!(f.messages.is_empty() && note.is_none());
        assert_eq!(f.version, VERSION);
    }

    #[test]
    fn bom_and_unknown_fields_are_fine() {
        let d = tmp("bom");
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("2.json");
        std::fs::write(&p, "\u{feff}{\"version\":1,\"future\":true,\"messages\":[{\"role\":\"user\",\"content\":\"x\",\"ts\":1,\"extra\":[1]}]}").unwrap();
        let (f, note) = load(&p);
        assert!(note.is_none());
        assert_eq!(f.messages.len(), 1);
        assert_eq!(f.messages[0].content, "x");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn broken_file_gives_empty_chat_warning_and_backup() {
        let d = tmp("broken");
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("3.json");
        std::fs::write(&p, "{ not json").unwrap();
        let (f, note) = load(&p);
        assert!(f.messages.is_empty());
        assert!(note.is_some());
        let backups = std::fs::read_dir(&d).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("3.defekt-")).count();
        assert_eq!(backups, 1, "beschädigte Datei wird gesichert");
        // Einzelne ungültige Nachricht: der Rest bleibt
        std::fs::write(&p, r#"{"messages":[{"role":"user","content":"a"},{"role":"wizard","content":"b"},7,{"role":"assistant","content":"c"}],"context_start":99}"#).unwrap();
        let (f, note) = load(&p);
        assert_eq!(f.messages.iter().map(|m| m.content.as_str()).collect::<Vec<_>>(), ["a", "c"]);
        assert!(note.unwrap().contains('2'));
        assert_eq!(f.context_start, None, "Index hinter dem Ende wird verworfen");
        // Kein Objekt
        assert!(parse("[1,2]").is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn size_cap_on_load_and_save() {
        let d = tmp("cap");
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("4.json");
        // Zu groß zum Laden
        let big = format!("{{\"messages\":[{{\"role\":\"user\",\"content\":\"{}\"}}]}}", "x".repeat(MAX_BYTES as usize + 10));
        std::fs::write(&p, big).unwrap();
        let (f, note) = load(&p);
        assert!(f.messages.is_empty() && note.is_some());
        // Speichern kürzt auf die Grenze, Systemnachricht bleibt
        let mut file = ChatFile::default();
        file.messages.push(ChatMsg::new(Role::System, "sys"));
        for i in 0..200 {
            file.messages.push(ChatMsg::new(Role::User, format!("{i}-{}", "y".repeat(100))));
        }
        file.context_start = Some(150);
        let data = serialize_capped(&file, 8_000);
        assert!(data.len() <= 8_000);
        let (back, _) = parse(std::str::from_utf8(&data).unwrap()).unwrap();
        assert_eq!(back.messages[0].role, Role::System);
        assert!(back.messages.len() < 201);
        assert!(back.messages.last().unwrap().content.starts_with("199-"), "die neuesten bleiben");
        assert!(data.len() > 6_000, "nicht mehr als nötig gekürzt");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn used_ids_orphans_and_removal() {
        let d = tmp("ids");
        std::fs::create_dir_all(dir(&d)).unwrap();
        std::fs::create_dir_all(d.join("scrollback")).unwrap();
        assert_eq!(max_used_id(&tmp("ids-none")), 0);
        std::fs::write(path(&d, 12), "{}").unwrap();
        std::fs::write(dir(&d).join("40.defekt-1.json"), "x").unwrap();
        std::fs::write(d.join("scrollback").join("77.txt"), "x").unwrap();
        std::fs::write(d.join("scrollback").join("notes.txt"), "x").unwrap();
        std::fs::write(d.join("scrollback").join("999"), "x").unwrap();
        assert_eq!(max_used_id(&d), 77);
        // Beiseitelegen: Datei weg, Inhalt in der Sicherung
        let b = set_aside(&path(&d, 12)).unwrap();
        assert!(!path(&d, 12).exists());
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "{}");
        assert!(set_aside(&path(&d, 12)).is_none());
        // Löschen: Verlauf und Sicherungen beschädigter Dateien, nicht beiseitegelegte fremde Verläufe
        std::fs::write(path(&d, 40), "{}").unwrap();
        remove(&d, 40);
        assert!(!path(&d, 40).exists() && !dir(&d).join("40.defekt-1.json").exists());
        assert!(b.exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
