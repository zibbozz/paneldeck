//! Austauschbares Backend eines Chats: erzeugt eine Antwort auf den bisherigen Verlauf und streamt sie über einen Kanal.
//!
//! Der Hub hält genau ein `Arc<dyn ChatBackend>` (`App::chat_backend`); welches Modell antwortet, entscheidet das Backend
//! anhand von [`ChatMeta`] (Server-Kennung + Modellname). Der nächste Teil ersetzt [`EchoBackend`] dort durch einen
//! Verteiler auf die echten Server (`src/llm/`).

use super::store::{ChatMsg, Role, ToolCall};
use crate::state::{ChatMeta, Lang};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

/// Neu zeichnen lassen (aus einem Hintergrund-Thread; im Hub `ctx.request_repaint()`).
pub type Repaint = Arc<dyn Fn() + Send + Sync>;

/// Ereignis einer laufenden Antwort.
#[derive(Clone, Debug, PartialEq)]
pub enum TurnEvent {
    /// Nächstes Stück Antworttext.
    Delta(String),
    /// Das Modell denkt noch (Denk-Ausgabe ohne Antworttext; der Text selbst wird nicht gezeigt) – die Oberfläche
    /// zeigt „Denkt nach …“ statt nur „…“ (#299).
    Thinking,
    /// Das Modell verlangt einen Werkzeugaufruf (kommt direkt vor `Done`; die Werkzeug-Schleife in `ChatState` führt
    /// ihn nach dem Ende der Antwort aus).
    ToolCall(ToolCall),
    /// Für diese Anfrage wurden ältere Nachrichten weggelassen: Index (im übergebenen Verlauf) der ersten, die das
    /// Modell noch sieht.
    ContextStart(usize),
    /// Kommt am Anfang der Antwort. `history`: Platz für den Verlauf in geschätzten Tokens (Kontext ohne
    /// Antwort-Reserve, Systemprompt und Werkzeug-Beschreibungen; damit hält die Werkzeug-Schleife Ergebnisse klein
    /// genug). `prompt`: geschätzte Tokens der ganzen Anfrage (Abgleich mit `Usage::prompt_tokens`).
    Budget { history: usize, prompt: usize },
    /// Aufteilung der Anfrage für die Anzeige des Füllstands (#339; kommt direkt nach `Budget`).
    Context(super::context::ContextParts),
    /// Verbrauch der Anfrage laut Server (Tokens der Eingabe und der Antwort), kurz vor `Done`.
    Usage { prompt_tokens: Option<u32>, completion_tokens: Option<u32> },
    /// Hinweis zur Antwort (z. B. „an der Token-Grenze abgeschnitten“), landet in [`ChatMsg::note`].
    Note(String),
    /// Hinweis „Kontext zu klein für die Werkzeug-Stufe“ (eigene Kontextlänge des Nutzers): landet wie `Note` an der
    /// Antwort, aber nur einmal je Chat.
    ContextNotice(String),
    /// Antwort fertig.
    Done,
    /// Abbruch mit Fehlermeldung (für den Nutzer, ein Satz).
    Error(String),
}

/// Laufende Antwort. Fallenlassen bricht sie ab (das Backend prüft [`TurnHandle::cancel`] regelmäßig).
pub struct TurnHandle {
    pub rx: Receiver<TurnEvent>,
    pub cancel: Arc<AtomicBool>,
}

impl TurnHandle {
    /// Kanal und Abbruch-Flag für ein Backend: (Handle für den Hub, Sender und Flag für den Thread).
    pub fn pair() -> (TurnHandle, Sender<TurnEvent>, Arc<AtomicBool>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        (TurnHandle { rx, cancel: cancel.clone() }, tx, cancel)
    }

    pub fn stop(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

impl Drop for TurnHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Was ein Backend außer dem Verlauf über den Chat und die Einstellungen wissen muss (eine Momentaufnahme je Anfrage).
#[derive(Clone, Debug)]
pub struct TurnContext {
    /// Projektname und Arbeitsordner des Chats (Kontextzeile für das Modell).
    pub project: String,
    pub folder: String,
    /// Server aus den Einstellungen (`Settings::llm_servers`).
    pub servers: Vec<crate::llm::config::LlmServer>,
    // Seit 2.6.1 (#340) ohne Datenordner und Prompt-Sprache: der Chat nutzt den Master-Prompt (immer Englisch), nicht
    // `system-prompt.md` und nicht die Einstellung „Sprache des Systemprompts“ (die gelten nur für externe Agenten).
    /// Sprache der Oberfläche (Fehlermeldungen, Hinweise).
    pub lang: Lang,
    /// Expertenmodus der Todo-Liste (bestimmt die Werkzeuge im Systemprompt).
    pub expert: bool,
    /// So viel mehr Tokens zählt der Server als geschätzt (aus der letzten Antwort des Chats, 1,0–4,0; 1,0 = unbekannt).
    pub token_scale: f32,
    /// Globale Schalter für die Teile des Master-Prompts (`Settings::chat_prompt_parts`, #341); je Modell abweichend in
    /// `LlmModel::prompt_parts`.
    pub prompt_parts: crate::llm::prompt_parts::PromptParts,
    /// Projektordner und Einstellungen der Agenten-Werkzeuge (#338; bestimmt, welche Werkzeuge die Anfrage anbietet).
    pub agent: super::agent::AgentEnv,
    /// Kompaktieren (#359): automatischer Modus, Schwelle, Sicherheitsfenster (`Settings::chat_compact`).
    pub compact: super::compact::CompactSettings,
}

impl Default for TurnContext {
    fn default() -> Self {
        TurnContext { project: String::new(), folder: String::new(), servers: Vec::new(), lang: Lang::En, expert: false, token_scale: 1.0, prompt_parts: Default::default(), agent: Default::default(), compact: Default::default() }
    }
}

/// Erzeugt Antworten. `history` ist der vollständige Verlauf bis einschließlich der letzten Nutzer-Nachricht; Nachrichten
/// mit `error` sind fehlgeschlagene Antworten und gehören nicht in die Anfrage (siehe [`super::logic::sendable`]).
pub trait ChatBackend: Send + Sync {
    fn start(&self, meta: &ChatMeta, history: &[ChatMsg], cx: &TurnContext, repaint: Repaint) -> TurnHandle;

    /// Aufteilung, die die nächste Anfrage mit diesem Verlauf hätte (#339/#359: Kontextlänge, Schwelle des
    /// automatischen Kompaktierens). `None` = unbekannt (dann wird nicht kompaktiert).
    fn estimate(&self, _meta: &ChatMeta, _history: &[ChatMsg], _cx: &TurnContext) -> Option<super::context::ContextParts> {
        None
    }

    /// Zusammenfassung für das Kompaktieren erzeugen (#359): dieselben Ereignisse wie [`ChatBackend::start`] (Text,
    /// `Done` oder `Error`), ohne Werkzeuge. Ohne eigene Umsetzung ein Fehler.
    fn summarize(&self, _meta: &ChatMeta, _req: &super::compact::SummaryRequest, cx: &TurnContext, repaint: Repaint) -> TurnHandle {
        let (h, tx, _) = TurnHandle::pair();
        let _ = tx.send(TurnEvent::Error(cx.lang.t("Dieses Backend kann nicht kompaktieren.", "This backend cannot compact chats.").into()));
        repaint();
        h
    }
}

/// Test- und Entwicklungs-Backend: antwortet Wort für Wort mit einem festen Text, der die letzte Nutzer-Nachricht zitiert.
/// Befehle in der Nachricht (zum Ausprobieren der Oberfläche): `/error` → Fehler, `/tool` → ein Werkzeugaufruf,
/// `/cut` → meldet, dass das Modell nur noch die letzte Nachricht sieht, `/slow` → langsamer streamen.
#[cfg_attr(not(test), allow(dead_code))]
pub struct EchoBackend {
    /// Pause zwischen zwei Wörtern.
    pub delay: Duration,
}

impl Default for EchoBackend {
    fn default() -> Self {
        EchoBackend { delay: Duration::from_millis(30) }
    }
}

#[cfg_attr(not(test), allow(dead_code))]
impl EchoBackend {
    /// Die feste Antwort auf `last` (Text der letzten Nutzer-Nachricht).
    pub fn reply(meta: &ChatMeta, last: &str) -> String {
        let quoted: String = last.lines().map(|l| format!("> {l}\n")).collect();
        format!("Echo from **{}**:\n\n{quoted}\n```text\n{}\n```\n", meta.model, last.trim())
    }
}

impl ChatBackend for EchoBackend {
    fn start(&self, meta: &ChatMeta, history: &[ChatMsg], _cx: &TurnContext, repaint: Repaint) -> TurnHandle {
        let (handle, tx, cancel) = TurnHandle::pair();
        // Wie ein echtes Backend: fehlgeschlagene Antworten gehören nicht in die Anfrage.
        let last = history.iter().filter(|m| super::logic::sendable(m)).rev().find(|m| m.role == Role::User).map(|m| m.content.clone()).unwrap_or_default();
        let n = history.len();
        // Werkzeugaufruf nur auf die Frage selbst, nicht noch einmal nach dem Ergebnis (sonst endlose Runde).
        let tool = last.contains("/tool") && history.last().is_some_and(|m| m.role == Role::User);
        let reply = Self::reply(meta, &last);
        let delay = if last.contains("/slow") { self.delay.max(Duration::from_millis(150)) } else { self.delay };
        std::thread::spawn(move || {
            let send = |ev: TurnEvent| {
                let ok = tx.send(ev).is_ok();
                repaint();
                ok
            };
            if last.contains("/error") {
                std::thread::sleep(delay);
                send(TurnEvent::Error("Echo backend: simulated error".into()));
                return;
            }
            if last.contains("/cut") && n > 0 {
                send(TurnEvent::ContextStart(n - 1));
            }
            if tool {
                send(TurnEvent::ToolCall(ToolCall { id: "echo-1".into(), name: "list_sessions".into(), arguments: serde_json::json!({"include_exited": false}) }));
            }
            // Wortweise, Leerraum bleibt am Wort (Zeilenumbrüche des Markdowns bleiben erhalten).
            let mut word = String::new();
            for c in reply.chars() {
                word.push(c);
                if c.is_whitespace() {
                    if cancel.load(Ordering::SeqCst) || !send(TurnEvent::Delta(std::mem::take(&mut word))) {
                        return;
                    }
                    if !delay.is_zero() {
                        std::thread::sleep(delay);
                    }
                }
            }
            if cancel.load(Ordering::SeqCst) {
                return;
            }
            if !word.is_empty() {
                send(TurnEvent::Delta(word));
            }
            send(TurnEvent::Done);
        });
        handle
    }

    /// Für Tests: 8192 Tokens Kontext, Verlauf = Summe der gesendeten Nachrichten.
    fn estimate(&self, _meta: &ChatMeta, history: &[ChatMsg], _cx: &TurnContext) -> Option<super::context::ContextParts> {
        let history = history.iter().filter(|m| super::logic::sendable(m) && !m.compacted).map(super::logic::msg_tokens).sum();
        Some(super::context::ContextParts { system: 100, tools: 0, history, max: 8192, reserve: 2048, dropped: false, scale: 1.0 })
    }

    /// „Summary of N messages.“ (N = Nachrichten im Stoff); `/error` im Stoff → Fehler.
    fn summarize(&self, _meta: &ChatMeta, req: &super::compact::SummaryRequest, _cx: &TurnContext, repaint: Repaint) -> TurnHandle {
        let (handle, tx, cancel) = TurnHandle::pair();
        let n = req.user.lines().filter(|l| l.starts_with("### ") || l.starts_with("← result")).count();
        let fail = req.user.contains("/error");
        let delay = self.delay;
        std::thread::spawn(move || {
            let send = |ev: TurnEvent| {
                let ok = tx.send(ev).is_ok();
                repaint();
                ok
            };
            std::thread::sleep(delay);
            if cancel.load(Ordering::SeqCst) {
                return;
            }
            if fail {
                send(TurnEvent::Error("Echo backend: simulated error".into()));
                return;
            }
            for w in format!("Summary of {n} messages. ").split_inclusive(' ') {
                if cancel.load(Ordering::SeqCst) || !send(TurnEvent::Delta(w.to_string())) {
                    return;
                }
            }
            send(TurnEvent::Done);
        });
        handle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(h: &TurnHandle) -> Vec<TurnEvent> {
        let mut v = Vec::new();
        while let Ok(ev) = h.rx.recv_timeout(Duration::from_secs(5)) {
            let end = matches!(ev, TurnEvent::Done | TurnEvent::Error(_));
            v.push(ev);
            if end {
                break;
            }
        }
        v
    }

    #[test]
    fn echo_streams_the_reply_word_by_word() {
        let meta = ChatMeta { server: "echo".into(), model: "m".into() };
        let b = EchoBackend { delay: Duration::ZERO };
        let h = b.start(&meta, &[ChatMsg::new(Role::User, "hello world")], &TurnContext::default(), Arc::new(|| {}));
        let ev = collect(&h);
        assert_eq!(ev.last(), Some(&TurnEvent::Done));
        let text: String = ev.iter().filter_map(|e| if let TurnEvent::Delta(d) = e { Some(d.as_str()) } else { None }).collect();
        assert_eq!(text, EchoBackend::reply(&meta, "hello world"));
        assert!(ev.iter().filter(|e| matches!(e, TurnEvent::Delta(_))).count() > 3, "mehrere Stücke");
    }

    #[test]
    fn echo_error_tool_and_cut() {
        let meta = ChatMeta::default();
        let b = EchoBackend { delay: Duration::ZERO };
        let ev = collect(&b.start(&meta, &[ChatMsg::new(Role::User, "x /error")], &TurnContext::default(), Arc::new(|| {})));
        assert!(matches!(ev.as_slice(), [TurnEvent::Error(_)]));
        let hist = [ChatMsg::new(Role::User, "a"), ChatMsg::new(Role::Assistant, "b"), ChatMsg::new(Role::User, "/tool /cut")];
        let ev = collect(&b.start(&meta, &hist, &TurnContext::default(), Arc::new(|| {})));
        assert_eq!(ev[0], TurnEvent::ContextStart(2));
        assert!(matches!(&ev[1], TurnEvent::ToolCall(c) if c.name == "list_sessions"));
    }

    #[test]
    fn dropping_the_handle_cancels() {
        let b = EchoBackend { delay: Duration::from_millis(20) };
        let h = b.start(&ChatMeta::default(), &[ChatMsg::new(Role::User, "a b c d e f g h i j k l m n o p")], &TurnContext::default(), Arc::new(|| {}));
        let flag = h.cancel.clone();
        let _ = h.rx.recv_timeout(Duration::from_secs(5));
        drop(h);
        assert!(flag.load(Ordering::SeqCst));
    }
}
