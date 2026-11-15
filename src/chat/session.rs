//! Zustand eines Chats zur Laufzeit (hängt an der Sitzung, nicht an einem Fenster): Verlauf, Eingabe, laufende Antwort.

use super::backend::{ChatBackend, Repaint, TurnEvent, TurnHandle};
use super::logic;
use super::store::{ChatFile, ChatMsg, Role};
use crate::state::{self, ChatMeta};
use std::cell::Cell;
use std::sync::mpsc::TryRecvError;

pub struct ChatState {
    /// Verlauf (wird als `chats/<id>.json` gespeichert).
    pub file: ChatFile,
    /// Text im Eingabefeld.
    pub draft: String,
    /// Die Eingabe bearbeitet die letzte eigene Nachricht (Senden ersetzt sie und verwirft die Antworten danach).
    pub editing: bool,
    /// Laufende Antwort.
    pub turn: Option<TurnHandle>,
    /// Index der Nachricht, in die die laufende Antwort schreibt.
    turn_msg: usize,
    /// Ans Ende scrollen, solange der Nutzer nicht hochgescrollt hat.
    pub follow: bool,
    /// Eingabefeld beim nächsten Zeichnen fokussieren.
    pub focus_input: bool,
    /// Hinweis vom Laden (beschädigte Datei o. Ä.).
    pub warning: Option<String>,
    /// Änderungszähler; `saved` merkt sich den zuletzt gespeicherten Stand (Speichern läuft über `&self`).
    rev: u64,
    saved: Cell<u64>,
    /// Geparste Antworten fürs Zeichnen: Index → (Inhalt-Hash, Dokument). Nur neu parsen, wenn sich der Text ändert.
    pub md_cache: std::collections::HashMap<usize, (u64, crate::markdown::Doc)>,
}

/// Ergebnis von [`ChatState::poll`].
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct Polled {
    /// Es kam etwas an.
    pub changed: bool,
    /// Die Antwort ist zu Ende (fertig, Fehler oder abgebrochen).
    pub finished: bool,
}

impl ChatState {
    pub fn new(file: ChatFile, warning: Option<String>) -> Self {
        ChatState { file, draft: String::new(), editing: false, turn: None, turn_msg: 0, follow: true, focus_input: false, warning, rev: 0, saved: Cell::new(0), md_cache: Default::default() }
    }

    pub fn messages(&self) -> &[ChatMsg] {
        &self.file.messages
    }

    pub fn streaming(&self) -> bool {
        self.turn.is_some()
    }

    /// Die letzte Antwort brach mit einem Fehler ab.
    pub fn has_error(&self) -> bool {
        !self.streaming() && self.file.messages.last().is_some_and(|m| m.role == Role::Assistant && m.error.is_some())
    }

    pub fn touch(&mut self) {
        self.rev += 1;
    }

    /// Ungespeicherte Änderungen?
    pub fn dirty(&self) -> bool {
        self.saved.get() != self.rev
    }

    pub fn mark_saved(&self) {
        self.saved.set(self.rev);
    }

    /// Eingabe senden (oder als Bearbeitung der letzten Nachricht übernehmen). `false`, wenn nichts zu senden ist oder
    /// gerade eine Antwort läuft.
    pub fn send(&mut self, backend: &dyn ChatBackend, meta: &ChatMeta, repaint: Repaint) -> bool {
        let text = self.draft.trim().to_string();
        if text.is_empty() || self.streaming() {
            return false;
        }
        if !(self.editing && logic::apply_edit(&mut self.file.messages, &text)) {
            self.file.messages.push(ChatMsg::new(Role::User, text));
        }
        self.editing = false;
        self.draft.clear();
        self.start_turn(backend, meta, repaint);
        true
    }

    /// Letzte Antwort verwerfen und neu erzeugen (auch „Wiederholen“ nach einem Fehler).
    pub fn regenerate(&mut self, backend: &dyn ChatBackend, meta: &ChatMeta, repaint: Repaint) -> bool {
        if self.streaming() || !logic::truncate_after_last_user(&mut self.file.messages) {
            return false;
        }
        self.start_turn(backend, meta, repaint);
        true
    }

    fn start_turn(&mut self, backend: &dyn ChatBackend, meta: &ChatMeta, repaint: Repaint) {
        self.file.context_start = logic::clamp_context_start(self.file.context_start, self.file.messages.len());
        let handle = backend.start(meta, &self.file.messages, repaint);
        self.file.messages.push(ChatMsg::new(Role::Assistant, ""));
        self.turn_msg = self.file.messages.len() - 1;
        self.turn = Some(handle);
        self.follow = true;
        self.touch();
    }

    /// Letzte eigene Nachricht zurück ins Eingabefeld holen.
    pub fn begin_edit(&mut self) -> bool {
        if self.streaming() {
            return false;
        }
        let Some(i) = logic::last_user(&self.file.messages) else { return false };
        self.draft = self.file.messages[i].content.clone();
        self.editing = true;
        self.focus_input = true;
        true
    }

    pub fn cancel_edit(&mut self) {
        if self.editing {
            self.editing = false;
            self.draft.clear();
        }
    }

    /// Laufende Antwort abbrechen; bis dahin Geschriebenes bleibt stehen, eine leere Antwort fällt weg.
    pub fn stop(&mut self) {
        if self.turn.take().is_none() {
            return;
        }
        self.drop_empty_answer();
        self.touch();
    }

    fn drop_empty_answer(&mut self) {
        let i = self.turn_msg;
        if i + 1 == self.file.messages.len() {
            let m = &self.file.messages[i];
            if m.role == Role::Assistant && m.content.is_empty() && m.tool_calls.is_empty() && m.error.is_none() {
                self.file.messages.pop();
            }
        }
    }

    /// Ereignisse der laufenden Antwort abholen (jedes Bild, für alle Chats).
    pub fn poll(&mut self) -> Polled {
        let mut p = Polled::default();
        let Some(turn) = &self.turn else { return p };
        let mut events = Vec::new();
        let mut gone = false;
        loop {
            match turn.rx.try_recv() {
                Ok(ev) => {
                    let end = matches!(ev, TurnEvent::Done | TurnEvent::Error(_));
                    events.push(ev);
                    if end {
                        break;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    gone = true;
                    break;
                }
            }
        }
        if gone && !events.iter().any(|e| matches!(e, TurnEvent::Done | TurnEvent::Error(_))) {
            events.push(TurnEvent::Error(state::t("Die Antwort brach unerwartet ab.", "The answer ended unexpectedly.").to_string()));
        }
        let i = self.turn_msg;
        for ev in events {
            p.changed = true;
            match ev {
                TurnEvent::Delta(d) => {
                    if let Some(m) = self.file.messages.get_mut(i) {
                        m.content += &d;
                    }
                }
                TurnEvent::ToolCall(c) => {
                    if let Some(m) = self.file.messages.get_mut(i) {
                        m.tool_calls.push(c);
                    }
                }
                TurnEvent::ContextStart(c) => self.file.context_start = logic::clamp_context_start(Some(c), i),
                TurnEvent::Done => {
                    self.turn = None;
                    p.finished = true;
                }
                TurnEvent::Error(e) => {
                    if let Some(m) = self.file.messages.get_mut(i) {
                        m.error = Some(e);
                    }
                    self.turn = None;
                    p.finished = true;
                }
            }
        }
        if p.changed {
            self.touch();
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::EchoBackend;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn pump(c: &mut ChatState) {
        let t = Instant::now();
        while c.streaming() && t.elapsed() < Duration::from_secs(5) {
            c.poll();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!c.streaming(), "Antwort endete nicht");
    }

    #[test]
    fn send_stream_edit_regenerate_and_error() {
        let b = EchoBackend { delay: Duration::ZERO };
        let meta = crate::chat::default_chat_meta();
        let rp: Repaint = Arc::new(|| {});
        let mut c = ChatState::new(ChatFile::default(), None);
        assert!(!c.send(&b, &meta, rp.clone()), "leere Eingabe sendet nichts");
        c.draft = "  hello  ".into();
        assert!(c.send(&b, &meta, rp.clone()));
        assert!(c.draft.is_empty() && c.streaming());
        c.draft = "second".into();
        assert!(!c.send(&b, &meta, rp.clone()), "während einer Antwort wird nicht gesendet");
        pump(&mut c);
        assert_eq!(c.messages().len(), 2);
        assert_eq!(c.messages()[0].content, "hello");
        assert_eq!(c.messages()[1].content, EchoBackend::reply(&meta, "hello"));
        assert!(c.dirty());
        c.mark_saved();
        assert!(!c.dirty());
        // Bearbeiten: ersetzt die Frage, verwirft die Antwort
        assert!(c.begin_edit());
        assert_eq!(c.draft, "hello");
        c.draft = "hello again".into();
        assert!(c.send(&b, &meta, rp.clone()));
        pump(&mut c);
        assert_eq!(c.messages().len(), 2);
        assert_eq!(c.messages()[1].content, EchoBackend::reply(&meta, "hello again"));
        // Neu erzeugen
        assert!(c.regenerate(&b, &meta, rp.clone()));
        pump(&mut c);
        assert_eq!(c.messages().len(), 2);
        // Fehler, dann Wiederholen
        c.draft = "x /error".into();
        c.send(&b, &meta, rp.clone());
        pump(&mut c);
        assert!(c.has_error());
        assert_eq!(c.messages().len(), 4);
        assert!(!logic::sendable(&c.messages()[3]));
        assert!(c.regenerate(&b, &meta, rp.clone()));
        pump(&mut c);
        assert!(c.has_error(), "Echo meldet bei /error wieder einen Fehler");
        assert_eq!(c.messages().len(), 4, "der alte Fehler wurde ersetzt, nicht angehängt");
    }

    #[test]
    fn stop_keeps_partial_text_and_drops_empty_answer() {
        let meta = crate::chat::default_chat_meta();
        let rp: Repaint = Arc::new(|| {});
        let slow = EchoBackend { delay: Duration::from_millis(30) };
        let mut c = ChatState::new(ChatFile::default(), None);
        c.draft = "one two three four five six seven eight nine ten".into();
        c.send(&slow, &meta, rp.clone());
        c.stop();
        assert!(!c.streaming());
        assert_eq!(c.messages().len(), 1, "leere Antwort fällt weg");
        c.draft = "one two three four five six seven eight nine ten".into();
        c.send(&slow, &meta, rp.clone());
        let t = Instant::now();
        while c.messages()[2].content.is_empty() && t.elapsed() < Duration::from_secs(5) {
            c.poll();
            std::thread::sleep(Duration::from_millis(2));
        }
        c.stop();
        let partial = c.messages()[2].content.clone();
        assert!(!partial.is_empty());
        std::thread::sleep(Duration::from_millis(80));
        c.poll();
        assert_eq!(c.messages()[2].content, partial, "nach Stopp kommt nichts mehr an");
    }

    #[test]
    fn backend_thread_dying_is_an_error() {
        struct Dead;
        impl ChatBackend for Dead {
            fn start(&self, _: &ChatMeta, _: &[ChatMsg], _: Repaint) -> TurnHandle {
                TurnHandle::pair().0
            }
        }
        let mut c = ChatState::new(ChatFile::default(), None);
        c.draft = "x".into();
        c.send(&Dead, &ChatMeta::default(), Arc::new(|| {}));
        let p = c.poll();
        assert!(p.finished && c.has_error());
    }

    #[test]
    fn tool_calls_and_context_start_are_recorded() {
        let b = EchoBackend { delay: Duration::ZERO };
        let meta = crate::chat::default_chat_meta();
        let mut c = ChatState::new(ChatFile::default(), None);
        for q in ["a", "b /tool /cut"] {
            c.draft = q.into();
            c.send(&b, &meta, Arc::new(|| {}));
            pump(&mut c);
        }
        assert_eq!(c.file.context_start, Some(2));
        assert_eq!(c.messages()[3].tool_calls[0].name, "list_sessions");
    }
}
