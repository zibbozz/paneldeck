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
//! * [`context`]: Füllstand des Kontexts (`4342/8192 (53%)`, #339) aus Schätzung und Zählung des Servers.
//! * [`agent`]: Agenten-Werkzeuge nur für Chats (#338): Dateien, Suche, Befehle im Terminal des Chats, Git, Web, To-dos.
//! * [`plan`]: Plan-Modus (#358) – nur lesend recherchieren, Plan mit Stufe low … max, Freigabe über die Plan-Karte.
//! * [`compact`]: Kompaktieren (#359) – ältere Nachrichten durch eine Zusammenfassung des Modells ersetzen, Sicherheitsfenster.
//! * [`images`]: Bilder in Chats (#357): Platzhalter `[Image#n]`, Ablage in `chats/<id>/images/`, Aufbereitung,
//!   Modelle ohne Bildverständnis, Bilder aus Werkzeug-Ergebnissen.
//! * [`bg_commands`]: Befehle im Hintergrund (#355), Zeitlimits und Wahl der Shell für `run_command`.
//! * [`ask`]: `ask_user` (#356) – Fragen mit Auswahl an den Nutzer (Prüfung der Argumente, Zustand des Formulars).

pub mod agent;
pub mod ask;
pub mod backend;
pub mod compact;
pub mod bg_commands;
pub mod context;
pub mod driver;
pub mod images;
pub mod llm_backend;
pub mod logic;
pub mod master_prompt;
pub mod plan;
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
