//! Chat-Sitzungen mit lokalen Modellen (#63): Nachrichten, Verlaufsdatei `chats/<id>.json`, austauschbares Backend
//! und der Zustand eines Chats zur Laufzeit. Kennt kein egui (die Oberfläche steht in `app/chat_ui.rs`).
//!
//! * [`store`]: Datenformat und tolerantes Laden/atomares Speichern der Verlaufsdatei (öffentliches Datenformat).
//! * [`backend`]: [`ChatBackend`] (eine Antwort erzeugen, gestreamt über [`TurnEvent`]) und [`EchoBackend`] für Tests.
//! * [`logic`]: reine Funktionen auf der Nachrichtenliste (Bearbeiten, Neu erzeugen, Wiederholen).
//! * [`session`]: [`ChatState`] – Nachrichten, Eingabe, laufende Antwort.

pub mod backend;
pub mod logic;
pub mod session;
pub mod store;

pub use backend::{ChatBackend, EchoBackend, Repaint};
pub use session::ChatState;
pub use store::{ChatFile, ChatMsg, Role, ToolStatus};

use crate::state::ChatMeta;

/// Server/Modell eines neuen Chats. Platzhalter, bis es einen Modell-Dialog gibt (der nächste Teil ersetzt das).
pub fn default_chat_meta() -> ChatMeta {
    ChatMeta { server: "echo".into(), model: "echo".into() }
}

/// Sekunden seit 1970 (Zeitstempel der Nachrichten).
pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
