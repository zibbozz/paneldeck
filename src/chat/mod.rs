//! Chat-Sitzungen mit lokalen Modellen (#63): Nachrichten, Verlaufsdatei `chats/<id>.json`, austauschbares Backend
//! und der Zustand eines Chats zur Laufzeit. Kennt kein egui (die Oberfläche steht in `app/chat_ui.rs`).
//!
//! * [`store`]: Datenformat und tolerantes Laden/atomares Speichern der Verlaufsdatei (öffentliches Datenformat).
//! * [`backend`]: [`ChatBackend`] (eine Antwort erzeugen, gestreamt über [`TurnEvent`]) und [`EchoBackend`] für Tests.
//! * [`logic`]: reine Funktionen auf der Nachrichtenliste (Bearbeiten, Neu erzeugen, Wiederholen).
//! * [`session`]: [`ChatState`] – Nachrichten, Eingabe, laufende Antwort, Werkzeug-Schleife.
//! * [`tools`]: welche Paneldeck-Werkzeuge ein Modell bekommt, Prüfung der Aufrufe, Ergebnis-Nachrichten.
//! * [`repair`]: als Text geschriebene Werkzeugaufrufe erkennen, Argumente normalisieren und prüfen (#340).
//! * [`master_prompt`]: der Systemprompt der Chats (#340) – Regeln, Paneldeck, Fehler, erzeugte Werkzeug-Dokumentation.
//! * [`driver`]: führt die Werkzeug-Schleife je Bild weiter, über [`driver::ChatToolHost`] (umgesetzt von der App).

pub mod backend;
pub mod driver;
pub mod llm_backend;
pub mod logic;
pub mod master_prompt;
pub mod repair;
pub mod session;
pub mod store;
pub mod tools;

#[cfg(test)]
pub use backend::EchoBackend;
pub use backend::{ChatBackend, Repaint, TurnContext};
pub use llm_backend::LlmBackend;
pub use session::{ChatState, ChatStatus, Decision, LevelAsker};
#[cfg(test)]
pub use session::ToolStep;
pub use store::{AllowRule, ChatFile, ChatMsg, Role, ToolStatus};

/// Server/Modell des Echo-Backends (nur Tests; Nutzer wählen das Modell im Dialog „Neuer Chat“).
#[cfg(test)]
pub fn default_chat_meta() -> crate::state::ChatMeta {
    use crate::state::ChatMeta;
    ChatMeta { server: "echo".into(), model: "echo".into() }
}

/// Sekunden seit 1970 (Zeitstempel der Nachrichten).
pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
