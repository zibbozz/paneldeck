//! Zustand eines Chats zur Laufzeit (hängt an der Sitzung, nicht an einem Fenster): Verlauf, Eingabe, laufende Antwort
//! und die Werkzeug-Schleife.
//!
//! Werkzeug-Schleife: Endet eine Antwort mit Werkzeugaufrufen, kommen diese in [`ChatState::calls`]. Der Hub fragt im
//! Hauptdurchgang jedes Bild [`ChatState::tool_step`] ab: Aufrufe werden der Reihe nach geprüft ([`ToolPolicy::check`]),
//! schreibende ggf. zur Freigabe vorgelegt, dann ausgeführt ([`ToolStep::Execute`], Ergebnis über einen Kanal). Sind
//! alle Ergebnisse da, folgt die nächste Anfrage ([`ToolStep::Request`]). Je Nutzer-Runde laufen höchstens
//! `max_steps` Aufrufe, danach hält die Runde an („Weiter“ gibt noch einmal so viele frei). Stopp, eine neue Nachricht
//! oder ein Neustart des Hubs geben offenen Aufrufen ein Ergebnis („abgebrochen“/„unterbrochen“), damit der Verlauf für
//! das Modell stimmig bleibt.

use super::backend::{ChatBackend, Repaint, TurnContext, TurnEvent, TurnHandle};
use super::logic;
use super::store::{ChatFile, ChatMsg, Role, ToolCall, ToolStatus};
use super::tools::{self, ToolPolicy, ToolResult};
use crate::state::{self, ChatMeta};
use std::cell::Cell;
use std::sync::mpsc::{Receiver, TryRecvError};

/// Zustand eines Werkzeugaufrufs der laufenden Runde.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallState {
    /// Wartet, bis die Aufrufe davor fertig sind.
    Queued,
    /// Wartet auf die Entscheidung des Nutzers (Erlauben / Immer / Ablehnen).
    Approval,
    /// Freigegeben, läuft beim nächsten Schritt.
    Approved,
    /// Läuft im Hub; das Ergebnis kommt über den Kanal.
    Running,
}

/// Offener Werkzeugaufruf.
pub struct PendingCall {
    pub call: ToolCall,
    pub state: CallState,
    rx: Option<Receiver<ToolResult>>,
}

/// Entscheidung des Nutzers zu einem schreibenden Aufruf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// Erlauben und dieses Werkzeug in diesem Chat nicht mehr nachfragen.
    Always,
    Deny,
}

/// Was der Hub für einen Chat als Nächstes tun soll.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolStep {
    /// Nichts (keine Runde, Aufruf läuft, Freigabe offen, angehalten).
    Idle,
    /// Diesen Aufruf ausführen (Name, Argumente als Objekt) und dann [`ChatState::tool_started`] aufrufen.
    Execute(ToolCall),
    /// Alle Ergebnisse sind da: die nächste Anfrage starten ([`ChatState::next_request`]).
    Request,
}

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
    /// Ab diesem Index haben sich Nachrichten seit dem letzten Zeichnen geändert (die Oberfläche setzt es zurück).
    pub changed_from: usize,
    /// Entwurf, der vor „Bearbeiten“ im Eingabefeld stand (kommt bei „Abbrechen“ zurück).
    draft_before_edit: Option<String>,
    /// Zwischenspeicher der Oberfläche (geparstes Markdown, Werkzeug-Zuordnung, Zeilenhöhen).
    pub ui: UiCache,
    /// Dauerhafter Hinweis oben im Chat (z. B. „Dieses Modell kann keine Werkzeuge nutzen“); setzt die App.
    pub notice: Option<String>,
    /// Token-Verbrauch der letzten Antwort (Eingabe, Antwort) laut Server; nicht gespeichert.
    pub usage: Option<(Option<u32>, Option<u32>)>,
    /// Offene Werkzeugaufrufe der laufenden Runde, in Reihenfolge (erledigte fallen weg).
    pub calls: Vec<PendingCall>,
    /// Nach den offenen Aufrufen folgt die nächste Anfrage an das Modell.
    next_request: bool,
    /// In dieser Nutzer-Runde ausgeführte Werkzeugaufrufe.
    pub steps: u32,
    /// Wie oft `max_steps` freigegeben ist (1 + „Weiter“).
    grants: u32,
    /// Die Runde hielt an der Höchstzahl der Aufrufe an (Knopf „Weiter“).
    pub paused: bool,
}

/// Was die Oberfläche je Chat zwischenspeichert, damit ein Bild nicht den ganzen Verlauf durchgeht.
#[derive(Default)]
pub struct UiCache {
    /// Geparste Antworten: Index → Dokument (neu geparst ab `changed_from`).
    pub md: std::collections::HashMap<usize, crate::markdown::Doc>,
    /// Stand (`rev`), für den `tool_results`/`matched` gelten.
    pub tool_rev: Option<u64>,
    /// Werkzeugaufruf-Id → Index der Ergebnis-Nachricht.
    pub tool_results: std::collections::HashMap<String, usize>,
    /// Ergebnis-Nachrichten, die in der Zeile ihres Aufrufs stehen (nicht einzeln zeichnen).
    pub matched: std::collections::HashSet<usize>,
    /// Zuletzt gemessene Zeilenhöhen je Nachricht (0 = noch nicht gemessen; sichtbare Zeilen werden jedes Bild neu gemessen).
    pub heights: Vec<f32>,
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
    /// Chat mit geladenem Verlauf. Aufrufe ohne Ergebnis (Hub endete mitten in einer Werkzeug-Runde) bekommen
    /// „unterbrochen“, damit die nächste Anfrage stimmig ist.
    pub fn new(mut file: ChatFile, warning: Option<String>) -> Self {
        let fixed = logic::close_orphans(&mut file.messages, tools::interrupted);
        let mut c = ChatState {
            file,
            draft: String::new(),
            editing: false,
            turn: None,
            turn_msg: 0,
            follow: true,
            focus_input: false,
            warning,
            rev: 0,
            saved: Cell::new(0),
            changed_from: 0,
            draft_before_edit: None,
            ui: UiCache::default(),
            notice: None,
            usage: None,
            calls: Vec::new(),
            next_request: false,
            steps: 0,
            grants: 1,
            paused: false,
        };
        if fixed > 0 {
            c.touch();
        }
        c
    }

    /// Änderungszähler (für Zwischenspeicher der Oberfläche).
    pub fn rev(&self) -> u64 {
        self.rev
    }

    /// Nachrichten ab `i` haben sich geändert.
    fn changed(&mut self, i: usize) {
        self.changed_from = self.changed_from.min(i);
        self.touch();
    }

    /// Was gespeichert wird: läuft gerade eine Antwort, ist sie in der Datei als unterbrochen markiert (endet der Hub
    /// vorher, zeigt der Chat nach dem Neustart „Wiederholen“). Die fertige Antwort überschreibt das später.
    /// Offene Werkzeugaufrufe stehen in der Datei mit dem Ergebnis „unterbrochen“ (im Speicher laufen sie weiter).
    pub fn file_for_save(&self) -> std::borrow::Cow<'_, ChatFile> {
        if !self.streaming() && self.calls.is_empty() {
            return std::borrow::Cow::Borrowed(&self.file);
        }
        let mut f = self.file.clone();
        if self.streaming() {
            if let Some(m) = f.messages.get_mut(self.turn_msg) {
                m.error = Some(state::t("unterbrochen", "interrupted").to_string());
            }
        }
        logic::close_orphans(&mut f.messages, tools::interrupted);
        std::borrow::Cow::Owned(f)
    }

    pub fn messages(&self) -> &[ChatMsg] {
        &self.file.messages
    }

    pub fn streaming(&self) -> bool {
        self.turn.is_some()
    }

    /// Die Runde läuft noch: Antwort wird erzeugt, Werkzeuge laufen oder warten auf Freigabe (nicht: angehalten).
    pub fn busy(&self) -> bool {
        self.streaming() || (!self.paused && (!self.calls.is_empty() || self.next_request))
    }

    /// Ein schreibender Aufruf wartet auf die Entscheidung des Nutzers.
    pub fn awaiting_approval(&self) -> bool {
        self.calls.iter().any(|c| c.state == CallState::Approval)
    }

    /// Ein Werkzeug läuft gerade im Hub.
    pub fn tool_running(&self) -> bool {
        self.calls.iter().any(|c| c.state == CallState::Running)
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
    pub fn send(&mut self, backend: &dyn ChatBackend, meta: &ChatMeta, cx: &TurnContext, repaint: Repaint) -> bool {
        let text = self.draft.trim().to_string();
        if text.is_empty() || self.busy() {
            return false;
        }
        // Eine angehaltene Runde endet: ihre offenen Aufrufe laufen nicht mehr.
        self.cancel_tools();
        self.reset_steps();
        let at = logic::last_user(&self.file.messages).filter(|_| self.editing).unwrap_or(self.file.messages.len());
        if !(self.editing && logic::apply_edit(&mut self.file.messages, &text)) {
            self.file.messages.push(ChatMsg::new(Role::User, text));
        }
        self.changed(at);
        self.editing = false;
        self.draft_before_edit = None;
        self.draft.clear();
        self.start_turn(backend, meta, cx, repaint);
        true
    }

    /// Letzte Antwort verwerfen und neu erzeugen (auch „Wiederholen“ nach einem Fehler).
    /// Mitten in einer Werkzeug-Runde (die fehlgeschlagene Antwort folgt auf Werkzeug-Ergebnisse) wiederholt es nur
    /// diesen Schritt, damit schon ausgeführte Werkzeuge nicht noch einmal laufen.
    pub fn regenerate(&mut self, backend: &dyn ChatBackend, meta: &ChatMeta, cx: &TurnContext, repaint: Repaint) -> bool {
        if self.busy() {
            return false;
        }
        let n = self.file.messages.len();
        let retry_step = self.has_error() && n >= 2 && self.file.messages[n - 2].role == Role::Tool;
        if retry_step {
            self.file.messages.pop();
            self.changed(n - 1);
            self.start_turn(backend, meta, cx, repaint);
            return true;
        }
        if !logic::truncate_after_last_user(&mut self.file.messages) {
            return false;
        }
        // Was angehalten auf „Weiter“ wartete, ist mit abgeschnitten.
        self.calls.clear();
        self.next_request = false;
        self.paused = false;
        self.reset_steps();
        self.changed(self.file.messages.len());
        self.start_turn(backend, meta, cx, repaint);
        true
    }

    /// Nächste Anfrage einer Werkzeug-Runde (nach [`ToolStep::Request`]).
    pub fn next_request(&mut self, backend: &dyn ChatBackend, meta: &ChatMeta, cx: &TurnContext, repaint: Repaint) {
        if !self.streaming() {
            self.start_turn(backend, meta, cx, repaint);
        }
    }

    fn reset_steps(&mut self) {
        self.steps = 0;
        self.grants = 1;
        self.paused = false;
    }

    fn start_turn(&mut self, backend: &dyn ChatBackend, meta: &ChatMeta, cx: &TurnContext, repaint: Repaint) {
        // Die Kürzung gilt nur für eine Anfrage; das Backend meldet sie neu (`TurnEvent::ContextStart`), wenn es kürzt.
        self.file.context_start = None;
        self.usage = None;
        let handle = backend.start(meta, &self.file.messages, cx, repaint);
        self.file.messages.push(ChatMsg::new(Role::Assistant, ""));
        self.turn_msg = self.file.messages.len() - 1;
        self.turn = Some(handle);
        self.follow = true;
        self.changed(self.turn_msg);
    }

    /// Letzte eigene Nachricht zurück ins Eingabefeld holen.
    pub fn begin_edit(&mut self) -> bool {
        if self.busy() {
            return false;
        }
        let Some(i) = logic::last_user(&self.file.messages) else { return false };
        if !self.editing {
            self.draft_before_edit = Some(std::mem::take(&mut self.draft));
        }
        self.draft = self.file.messages[i].content.clone();
        self.editing = true;
        self.focus_input = true;
        true
    }

    /// Bearbeiten abbrechen: der vorherige Entwurf kommt zurück.
    pub fn cancel_edit(&mut self) {
        if self.editing {
            self.editing = false;
            self.draft = self.draft_before_edit.take().unwrap_or_default();
        }
    }

    /// Laufende Antwort abbrechen; bis dahin Geschriebenes bleibt stehen, eine leere Antwort fällt weg. Was schon
    /// angekommen ist, wird vorher abgeholt – ist die Antwort dabei schon fertig, bleibt sie vollständig. Offene
    /// Werkzeugaufrufe (auch ein laufender) bekommen das Ergebnis „abgebrochen“; ein schon gestartetes Werkzeug kann im
    /// Hub trotzdem noch zu Ende laufen, sein Ergebnis wird verworfen.
    pub fn stop(&mut self) {
        if self.streaming() && !self.poll().finished && self.turn.take().is_some() {
            self.drop_empty_answer();
            self.changed(self.turn_msg);
        }
        self.cancel_tools();
    }

    /// Alle offenen Aufrufe beenden (Ergebnis „abgebrochen“) und die Runde schließen.
    fn cancel_tools(&mut self) {
        let had = !self.calls.is_empty() || self.next_request || self.paused;
        for p in std::mem::take(&mut self.calls) {
            let (st, content) = tools::cancelled();
            self.file.messages.push(logic::tool_result(&p.call, st, content));
        }
        // Auch Aufrufe einer abgebrochenen Antwort, die noch nicht in `calls` standen.
        let fixed = logic::close_orphans(&mut self.file.messages, tools::cancelled);
        self.next_request = false;
        self.paused = false;
        if had || fixed > 0 {
            self.changed(self.turn_msg.min(self.file.messages.len()));
        }
    }

    /// Ergebnis des ersten offenen Aufrufs eintragen und ihn entfernen.
    fn finish_first(&mut self, status: ToolStatus, content: String) {
        if self.calls.is_empty() {
            return;
        }
        let p = self.calls.remove(0);
        let at = self.file.messages.len();
        self.file.messages.push(logic::tool_result(&p.call, status, content));
        self.changed(at);
    }

    /// Nächster Schritt der Werkzeug-Schleife (im Hauptdurchgang, jedes Bild). Ungültige oder verbotene Aufrufe
    /// bekommen hier gleich ihr Fehler-Ergebnis; die Einstellungen werden bei jedem Aufruf neu geprüft.
    pub fn tool_step(&mut self, policy: &ToolPolicy) -> ToolStep {
        if self.streaming() || self.paused {
            return ToolStep::Idle;
        }
        loop {
            let Some(p) = self.calls.first() else {
                if std::mem::take(&mut self.next_request) {
                    return ToolStep::Request;
                }
                return ToolStep::Idle;
            };
            match p.state {
                CallState::Running | CallState::Approval => return ToolStep::Idle,
                CallState::Queued | CallState::Approved => {}
            }
            let approved = p.state == CallState::Approved;
            let write = match policy.check(&p.call) {
                Ok(w) => w,
                Err((st, content)) => {
                    self.finish_first(st, content);
                    continue;
                }
            };
            if self.steps >= policy.cfg.max_steps.max(1).saturating_mul(self.grants) {
                self.paused = true;
                self.touch();
                return ToolStep::Idle;
            }
            if !approved && policy.needs_approval(write, &p.call.name, &self.file.always_allow) {
                self.calls[0].state = CallState::Approval;
                self.touch();
                return ToolStep::Idle;
            }
            self.steps += 1;
            self.calls[0].state = CallState::Running;
            self.touch();
            let c = &self.calls[0].call;
            return ToolStep::Execute(ToolCall { id: c.id.clone(), name: c.name.clone(), arguments: tools::exec_args(c) });
        }
    }

    /// Der Hub hat den Aufruf aus [`ToolStep::Execute`] gestartet; das Ergebnis kommt über `rx`.
    pub fn tool_started(&mut self, rx: Receiver<ToolResult>) {
        if let Some(p) = self.calls.first_mut().filter(|p| p.state == CallState::Running) {
            p.rx = Some(rx);
        }
    }

    /// Ergebnis eines laufenden Aufrufs abholen. `true`, wenn sich etwas geändert hat.
    pub fn poll_tools(&mut self) -> bool {
        let Some(p) = self.calls.first().filter(|p| p.state == CallState::Running) else { return false };
        let Some(rx) = &p.rx else { return false };
        let r = match rx.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => Err(("internal".into(), "The tool ended without a result.".into())),
        };
        let name = p.call.name.clone();
        let (st, content) = tools::result_message(&name, r);
        self.finish_first(st, content);
        true
    }

    /// Entscheidung des Nutzers zum wartenden Aufruf. `false`, wenn keiner wartet.
    pub fn decide(&mut self, d: Decision) -> bool {
        let Some(i) = self.calls.iter().position(|c| c.state == CallState::Approval) else { return false };
        match d {
            Decision::Allow => self.calls[i].state = CallState::Approved,
            Decision::Always => {
                let name = self.calls[i].call.name.clone();
                if !self.file.always_allow.contains(&name) {
                    self.file.always_allow.push(name);
                }
                self.calls[i].state = CallState::Approved;
            }
            Decision::Deny => {
                // Nur der erste Aufruf kann warten (Reihenfolge), also ist `i == 0`.
                let (st, content) = tools::denied();
                self.finish_first(st, content);
            }
        }
        self.touch();
        true
    }

    /// „Weiter“ nach dem Anhalten an der Höchstzahl: noch einmal so viele Aufrufe freigeben.
    pub fn resume(&mut self) -> bool {
        if !self.paused {
            return false;
        }
        self.paused = false;
        self.grants += 1;
        self.touch();
        true
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
                TurnEvent::ToolCall(mut c) => {
                    // Kennungen müssen im ganzen Chat eindeutig sein (Ollama vergibt je Antwort `call_0`, `call_1` …).
                    let taken = |id: &str, msgs: &[ChatMsg]| id.is_empty() || msgs.iter().any(|m| m.tool_calls.iter().any(|x| x.id == id));
                    if taken(&c.id, &self.file.messages) {
                        let mut k = self.file.messages.get(i).map_or(0, |m| m.tool_calls.len());
                        loop {
                            let id = format!("call_{i}_{k}");
                            if !taken(&id, &self.file.messages) {
                                c.id = id;
                                break;
                            }
                            k += 1;
                        }
                    }
                    if let Some(m) = self.file.messages.get_mut(i) {
                        m.tool_calls.push(c);
                    }
                }
                TurnEvent::ContextStart(c) => self.file.context_start = logic::clamp_context_start(Some(c), i),
                TurnEvent::Usage { prompt_tokens, completion_tokens } => self.usage = Some((prompt_tokens, completion_tokens)),
                TurnEvent::Note(n) => {
                    if let Some(m) = self.file.messages.get_mut(i) {
                        m.note = Some(n);
                    }
                }
                TurnEvent::Done => {
                    self.turn = None;
                    p.finished = true;
                    // Werkzeugaufrufe: die Runde geht weiter (der Hub führt sie aus, dann folgt die nächste Anfrage).
                    if let Some(m) = self.file.messages.get(i).filter(|m| m.error.is_none() && !m.tool_calls.is_empty()) {
                        self.calls = m.tool_calls.iter().map(|c| PendingCall { call: c.clone(), state: CallState::Queued, rx: None }).collect();
                        self.next_request = true;
                    }
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
            self.changed(i);
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
        assert!(!c.send(&b, &meta, &TurnContext::default(), rp.clone()), "leere Eingabe sendet nichts");
        c.draft = "  hello  ".into();
        assert!(c.send(&b, &meta, &TurnContext::default(), rp.clone()));
        assert!(c.draft.is_empty() && c.streaming());
        c.draft = "second".into();
        assert!(!c.send(&b, &meta, &TurnContext::default(), rp.clone()), "während einer Antwort wird nicht gesendet");
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
        assert!(c.send(&b, &meta, &TurnContext::default(), rp.clone()));
        pump(&mut c);
        assert_eq!(c.messages().len(), 2);
        assert_eq!(c.messages()[1].content, EchoBackend::reply(&meta, "hello again"));
        // Neu erzeugen
        assert!(c.regenerate(&b, &meta, &TurnContext::default(), rp.clone()));
        pump(&mut c);
        assert_eq!(c.messages().len(), 2);
        // Fehler, dann Wiederholen
        c.draft = "x /error".into();
        c.send(&b, &meta, &TurnContext::default(), rp.clone());
        pump(&mut c);
        assert!(c.has_error());
        assert_eq!(c.messages().len(), 4);
        assert!(!logic::sendable(&c.messages()[3]));
        assert!(c.regenerate(&b, &meta, &TurnContext::default(), rp.clone()));
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
        c.send(&slow, &meta, &TurnContext::default(), rp.clone());
        c.stop();
        assert!(!c.streaming());
        assert_eq!(c.messages().len(), 1, "leere Antwort fällt weg");
        c.draft = "one two three four five six seven eight nine ten".into();
        c.send(&slow, &meta, &TurnContext::default(), rp.clone());
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
    fn context_start_is_reset_for_every_request() {
        let b = EchoBackend { delay: Duration::ZERO };
        let meta = crate::chat::default_chat_meta();
        let mut c = ChatState::new(ChatFile::default(), None);
        for q in ["a", "b /cut"] {
            c.draft = q.into();
            c.send(&b, &meta, &TurnContext::default(), Arc::new(|| {}));
            pump(&mut c);
        }
        assert_eq!(c.file.context_start, Some(2));
        c.draft = "c".into();
        c.send(&b, &meta, &TurnContext::default(), Arc::new(|| {}));
        assert_eq!(c.file.context_start, None, "alte Kürzung gilt nicht für die neue Anfrage");
        pump(&mut c);
        assert_eq!(c.file.context_start, None);
        // Neu erzeugen ebenso
        c.file.context_start = Some(1);
        c.regenerate(&b, &meta, &TurnContext::default(), Arc::new(|| {}));
        assert_eq!(c.file.context_start, None);
    }

    #[test]
    fn stop_after_the_answer_is_already_done_keeps_it_complete() {
        let b = EchoBackend { delay: Duration::ZERO };
        let meta = crate::chat::default_chat_meta();
        let mut c = ChatState::new(ChatFile::default(), None);
        c.draft = "hello there".into();
        c.send(&b, &meta, &TurnContext::default(), Arc::new(|| {}));
        // Warten, bis der Thread alles geschickt hat (Done liegt im Kanal), aber nicht abholen
        std::thread::sleep(Duration::from_millis(200));
        c.stop();
        assert!(!c.streaming());
        assert_eq!(c.messages()[1].content, EchoBackend::reply(&meta, "hello there"), "fertige Antwort geht beim Stopp nicht verloren");
        assert!(c.messages()[1].error.is_none());
    }

    #[test]
    fn cancel_edit_restores_the_previous_draft() {
        let b = EchoBackend { delay: Duration::ZERO };
        let meta = crate::chat::default_chat_meta();
        let mut c = ChatState::new(ChatFile::default(), None);
        c.draft = "q".into();
        c.send(&b, &meta, &TurnContext::default(), Arc::new(|| {}));
        pump(&mut c);
        c.draft = "half typed".into();
        assert!(c.begin_edit());
        assert_eq!(c.draft, "q");
        c.cancel_edit();
        assert_eq!(c.draft, "half typed");
    }

    #[test]
    fn saving_while_streaming_marks_the_answer_interrupted() {
        let slow = EchoBackend { delay: Duration::from_millis(50) };
        let meta = crate::chat::default_chat_meta();
        let mut c = ChatState::new(ChatFile::default(), None);
        assert!(matches!(c.file_for_save(), std::borrow::Cow::Borrowed(_)));
        c.draft = "a b c d e f".into();
        c.send(&slow, &meta, &TurnContext::default(), Arc::new(|| {}));
        let f = c.file_for_save();
        assert_eq!(f.messages[1].error.as_deref(), Some("interrupted"));
        assert!(c.messages()[1].error.is_none(), "nur die gespeicherte Fassung ist markiert");
    }

    #[test]
    fn backend_thread_dying_is_an_error() {
        struct Dead;
        impl ChatBackend for Dead {
            fn start(&self, _: &ChatMeta, _: &[ChatMsg], _: &TurnContext, _: Repaint) -> TurnHandle {
                TurnHandle::pair().0
            }
        }
        let mut c = ChatState::new(ChatFile::default(), None);
        c.draft = "x".into();
        c.send(&Dead, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {}));
        let p = c.poll();
        assert!(p.finished && c.has_error());
    }

    #[test]
    fn usage_and_note_are_recorded() {
        struct Canned;
        impl ChatBackend for Canned {
            fn start(&self, _: &ChatMeta, _: &[ChatMsg], _: &TurnContext, _: Repaint) -> TurnHandle {
                let (h, tx, _) = TurnHandle::pair();
                for ev in [TurnEvent::Delta("x".into()), TurnEvent::Usage { prompt_tokens: Some(9), completion_tokens: None }, TurnEvent::Note("cut".into()), TurnEvent::Done] {
                    tx.send(ev).unwrap();
                }
                h
            }
        }
        let mut c = ChatState::new(ChatFile::default(), None);
        c.draft = "q".into();
        c.send(&Canned, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {}));
        assert!(c.poll().finished);
        assert_eq!(c.usage, Some((Some(9), None)));
        assert_eq!(c.messages()[1].note.as_deref(), Some("cut"));
        assert!(serde_json::to_string(&c.file).unwrap().contains("\"note\":\"cut\""), "Hinweis wird gespeichert");
        c.draft = "q2".into();
        c.send(&Canned, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {}));
        assert_eq!(c.usage, None, "neue Anfrage: alter Verbrauch weg");
    }

    /// Antwortet nach einem Drehbuch: je Anfrage die nächste Liste von Ereignissen (die letzte wiederholt sich).
    struct Script(std::sync::Mutex<Vec<Vec<TurnEvent>>>);

    impl Script {
        fn new(turns: Vec<Vec<TurnEvent>>) -> Self {
            Script(std::sync::Mutex::new(turns))
        }
    }

    impl ChatBackend for Script {
        fn start(&self, _: &ChatMeta, _: &[ChatMsg], _: &TurnContext, _: Repaint) -> TurnHandle {
            let (h, tx, _) = TurnHandle::pair();
            let mut v = self.0.lock().unwrap();
            let evs = if v.len() > 1 { v.remove(0) } else { v[0].clone() };
            for ev in evs {
                tx.send(ev).unwrap();
            }
            h
        }
    }

    fn tc(id: &str, name: &str, args: serde_json::Value) -> TurnEvent {
        TurnEvent::ToolCall(ToolCall { id: id.into(), name: name.into(), arguments: args })
    }

    fn policy(level: crate::mcp::McpLevel, max_steps: u32) -> ToolPolicy {
        ToolPolicy::new(crate::llm::config::LlmModel { level, max_steps, ..crate::llm::config::LlmModel::named("m") }, None, false)
    }

    /// Hub nachspielen: Antwort abholen, Werkzeuge mit `exec` ausführen, nächste Anfrage, bis nichts mehr geschieht.
    /// Gibt die Namen der ausgeführten Werkzeuge zurück.
    fn drive(c: &mut ChatState, b: &dyn ChatBackend, p: &ToolPolicy, exec: &dyn Fn(&ToolCall) -> ToolResult) -> Vec<String> {
        let mut ran = Vec::new();
        for _ in 0..200 {
            c.poll();
            c.poll_tools();
            match c.tool_step(p) {
                ToolStep::Idle if !c.streaming() && !c.tool_running() => break,
                ToolStep::Idle => {}
                ToolStep::Execute(call) => {
                    let (tx, rx) = std::sync::mpsc::sync_channel(1);
                    tx.send(exec(&call)).unwrap();
                    c.tool_started(rx);
                    ran.push(call.name);
                }
                ToolStep::Request => c.next_request(b, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {})),
            }
        }
        ran
    }

    fn ok_exec(_: &ToolCall) -> ToolResult {
        Ok(serde_json::json!({"sessions": []}))
    }

    fn consistent(c: &ChatState) {
        let mut msgs = c.messages().to_vec();
        assert_eq!(logic::close_orphans(&mut msgs, tools::interrupted), 0, "jeder Aufruf hat ein Ergebnis: {:#?}", c.messages());
    }

    #[test]
    fn tool_loop_executes_calls_then_asks_again() {
        let b = Script::new(vec![vec![TurnEvent::Delta("Let me check.".into()), tc("call_0", "list_sessions", serde_json::json!({})), TurnEvent::Done], vec![TurnEvent::Delta("No sessions.".into()), TurnEvent::Done]]);
        let mut c = ChatState::new(ChatFile::default(), None);
        c.draft = "which sessions?".into();
        assert!(c.send(&b, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {})));
        let ran = drive(&mut c, &b, &policy(crate::mcp::McpLevel::Read, 10), &ok_exec);
        assert_eq!(ran, ["list_sessions"]);
        let roles: Vec<Role> = c.messages().iter().map(|m| m.role).collect();
        assert_eq!(roles, [Role::User, Role::Assistant, Role::Tool, Role::Assistant]);
        assert_eq!(c.messages()[2].status, Some(ToolStatus::Ok));
        assert_eq!(c.messages()[2].content, r#"{"sessions":[]}"#);
        assert_eq!(c.messages()[2].tool_call_id.as_deref(), Some("call_0"));
        assert_eq!(c.messages()[3].content, "No sessions.");
        assert!(!c.busy() && c.steps == 1);
        consistent(&c);
    }

    #[test]
    fn duplicate_call_ids_are_made_unique() {
        let b = Script::new(vec![vec![tc("call_0", "list_sessions", serde_json::json!({})), TurnEvent::Done], vec![tc("call_0", "whoami", serde_json::json!({})), TurnEvent::Done], vec![TurnEvent::Delta("ok".into()), TurnEvent::Done]]);
        let mut c = ChatState::new(ChatFile::default(), None);
        c.draft = "q".into();
        c.send(&b, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {}));
        assert_eq!(drive(&mut c, &b, &policy(crate::mcp::McpLevel::Read, 10), &ok_exec), ["list_sessions", "whoami"]);
        let ids: Vec<&str> = c.messages().iter().flat_map(|m| m.tool_calls.iter().map(|t| t.id.as_str())).collect();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);
        assert_eq!(c.messages()[4].tool_call_id.as_deref(), Some(ids[1]));
        consistent(&c);
    }

    #[test]
    fn invalid_unknown_and_forbidden_calls_get_errors_without_running() {
        let b = Script::new(vec![
            vec![tc("a", "list_sessions", serde_json::Value::String("{broken".into())), tc("b", "format_disk", serde_json::json!({})), tc("c", "send_text", serde_json::json!({"session": 1, "text": "x"})), TurnEvent::Done],
            vec![TurnEvent::Delta("sorry".into()), TurnEvent::Done],
        ]);
        let mut c = ChatState::new(ChatFile::default(), None);
        c.draft = "q".into();
        c.send(&b, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {}));
        let ran = drive(&mut c, &b, &policy(crate::mcp::McpLevel::Read, 10), &|_| panic!("darf nicht laufen"));
        assert!(ran.is_empty());
        let res: Vec<(Option<ToolStatus>, &str)> = c.messages().iter().filter(|m| m.role == Role::Tool).map(|m| (m.status, m.content.as_str())).collect();
        assert_eq!(res.len(), 3);
        assert!(res[0].1.contains("arguments must be a JSON object") && res[0].0 == Some(ToolStatus::Error));
        assert!(res[1].1.contains("unknown_tool"));
        assert!(res[2].1.contains("forbidden") && res[2].0 == Some(ToolStatus::Denied), "Lesen: send_text verboten");
        assert_eq!(c.messages().last().unwrap().content, "sorry", "das Modell bekommt die Fehler und antwortet");
        assert_eq!(c.steps, 0, "abgewiesene Aufrufe zählen nicht");
        consistent(&c);
    }

    #[test]
    fn step_limit_pauses_and_continue_grants_more() {
        // Das Modell ruft immer wieder dasselbe Werkzeug auf.
        let b = Script::new(vec![vec![tc("x", "list_sessions", serde_json::json!({})), TurnEvent::Done]]);
        let p = policy(crate::mcp::McpLevel::Read, 3);
        let mut c = ChatState::new(ChatFile::default(), None);
        c.draft = "loop".into();
        c.send(&b, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {}));
        assert_eq!(drive(&mut c, &b, &p, &|_| Err(("boom".into(), "fails".into()))).len(), 3);
        assert!(c.paused && !c.busy() && c.steps == 3);
        assert_eq!(c.calls.len(), 1, "der vierte Aufruf wartet auf „Weiter“");
        // Gespeichert wird stimmig (der wartende Aufruf als unterbrochen)
        let mut saved = c.file_for_save().into_owned().messages;
        assert_eq!(logic::close_orphans(&mut saved, tools::interrupted), 0);
        assert!(c.resume());
        assert_eq!(drive(&mut c, &b, &p, &ok_exec).len(), 3, "„Weiter“ gibt noch einmal so viele frei");
        assert!(c.paused && c.steps == 6);
        // Neue Nachricht: der wartende Aufruf wird abgebrochen, die Zählung beginnt neu.
        c.draft = "enough".into();
        let b2 = Script::new(vec![vec![TurnEvent::Delta("ok".into()), TurnEvent::Done]]);
        assert!(c.send(&b2, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {})));
        assert!(!c.paused && c.steps == 0 && c.calls.is_empty());
        drive(&mut c, &b2, &p, &ok_exec);
        consistent(&c);
        let cancelled = c.messages().iter().filter(|m| m.content.contains("\"cancelled\"")).count();
        assert_eq!(cancelled, 1);
    }

    #[test]
    fn write_tools_wait_for_approval_deny_and_always() {
        let call = || tc("w", "send_text", serde_json::json!({"session": 2, "text": "dir"}));
        let b = Script::new(vec![vec![call(), TurnEvent::Done], vec![TurnEvent::Delta("done".into()), TurnEvent::Done]]);
        let p = policy(crate::mcp::McpLevel::Control, 10);
        let mut c = ChatState::new(ChatFile::default(), None);
        c.draft = "type dir".into();
        c.send(&b, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {}));
        assert!(drive(&mut c, &b, &p, &|_| panic!("erst nach Freigabe")).is_empty());
        assert!(c.awaiting_approval() && c.busy());
        assert!(!c.send(&b, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {})), "während der Frage wird nicht gesendet");
        // Ablehnen
        assert!(c.decide(Decision::Deny));
        drive(&mut c, &b, &p, &|_| panic!("abgelehnt"));
        let t = c.messages().iter().find(|m| m.role == Role::Tool).unwrap();
        assert_eq!(t.status, Some(ToolStatus::Denied));
        assert!(t.content.contains("\"denied\""));
        consistent(&c);
        // Immer für diesen Chat: läuft, und beim nächsten Mal ohne Frage
        let b = Script::new(vec![vec![call(), TurnEvent::Done], vec![TurnEvent::Delta("a".into()), TurnEvent::Done], vec![call(), TurnEvent::Done], vec![TurnEvent::Delta("b".into()), TurnEvent::Done]]);
        c.draft = "again".into();
        c.send(&b, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {}));
        drive(&mut c, &b, &p, &|_| panic!("erst nach Freigabe"));
        assert!(c.decide(Decision::Always));
        assert_eq!(drive(&mut c, &b, &p, &ok_exec), ["send_text"]);
        assert_eq!(c.file.always_allow, ["send_text"]);
        c.draft = "third".into();
        c.send(&b, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {}));
        assert_eq!(drive(&mut c, &b, &p, &ok_exec), ["send_text"], "ohne Nachfrage");
        assert!(!c.awaiting_approval());
        // Erlauben (einmal) – und gespeichert/geladen bleibt „immer“ erhalten
        let back = super::super::store::parse(std::str::from_utf8(&super::super::store::serialize_capped(&c.file, 1 << 20)).unwrap()).unwrap().0;
        assert_eq!(back.always_allow, ["send_text"]);
    }

    #[test]
    fn stop_cancels_pending_and_running_calls() {
        let b = Script::new(vec![vec![tc("a", "send_text", serde_json::json!({"session": 2, "text": "x"})), tc("b", "list_sessions", serde_json::json!({})), TurnEvent::Done]]);
        let p = policy(crate::mcp::McpLevel::Control, 10);
        let mut c = ChatState::new(ChatFile::default(), None);
        c.draft = "q".into();
        c.send(&b, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {}));
        drive(&mut c, &b, &p, &ok_exec);
        assert!(c.awaiting_approval());
        c.stop();
        assert!(!c.busy() && c.calls.is_empty());
        let st: Vec<_> = c.messages().iter().filter(|m| m.role == Role::Tool).map(|m| m.status).collect();
        assert_eq!(st, [Some(ToolStatus::Denied), Some(ToolStatus::Denied)]);
        consistent(&c);
        // Laufendes Werkzeug: Stopp gibt sofort „abgebrochen“, das spätere Ergebnis wird verworfen
        let b = Script::new(vec![vec![tc("c", "list_sessions", serde_json::json!({})), TurnEvent::Done]]);
        c.draft = "q2".into();
        c.send(&b, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {}));
        c.poll();
        let ToolStep::Execute(_) = c.tool_step(&p) else { panic!() };
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        c.tool_started(rx);
        assert!(c.tool_running());
        c.stop();
        let _ = tx.send(Ok(serde_json::json!({})));
        assert!(!c.poll_tools() && !c.busy());
        consistent(&c);
        // Stopp mitten im Strom mit schon gemeldetem Aufruf: auch dieser bekommt ein Ergebnis.
        struct Hang;
        impl ChatBackend for Hang {
            fn start(&self, _: &ChatMeta, _: &[ChatMsg], _: &TurnContext, _: Repaint) -> TurnHandle {
                let (h, tx, _) = TurnHandle::pair();
                tx.send(TurnEvent::ToolCall(ToolCall { id: "h".into(), name: "list_sessions".into(), arguments: serde_json::json!({}) })).unwrap();
                std::mem::forget(tx); // kein Done
                h
            }
        }
        c.draft = "q3".into();
        c.send(&Hang, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {}));
        c.poll();
        c.stop();
        consistent(&c);
    }

    #[test]
    fn retry_after_an_error_mid_loop_repeats_only_that_step() {
        let b = Script::new(vec![vec![tc("a", "list_sessions", serde_json::json!({})), TurnEvent::Done], vec![TurnEvent::Error("server gone".into())], vec![TurnEvent::Delta("fine".into()), TurnEvent::Done]]);
        let p = policy(crate::mcp::McpLevel::Read, 10);
        let mut c = ChatState::new(ChatFile::default(), None);
        c.draft = "q".into();
        c.send(&b, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {}));
        assert_eq!(drive(&mut c, &b, &p, &ok_exec).len(), 1);
        assert!(c.has_error());
        assert!(c.regenerate(&b, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {})));
        assert!(drive(&mut c, &b, &p, &|_| panic!("Werkzeug lief schon")).is_empty());
        let roles: Vec<Role> = c.messages().iter().map(|m| m.role).collect();
        assert_eq!(roles, [Role::User, Role::Assistant, Role::Tool, Role::Assistant]);
        assert_eq!(c.messages()[3].content, "fine");
    }

    #[test]
    fn loading_a_history_with_open_calls_marks_them_interrupted() {
        let mut a = ChatMsg::new(Role::Assistant, "");
        a.tool_calls.push(ToolCall { id: "z".into(), name: "run_command".into(), arguments: serde_json::json!({}) });
        let c = ChatState::new(ChatFile { messages: vec![ChatMsg::new(Role::User, "q"), a], ..ChatFile::default() }, None);
        assert_eq!(c.messages().len(), 3);
        assert!(c.messages()[2].content.contains("\"interrupted\""));
        assert_eq!(c.messages()[2].status, Some(ToolStatus::Error));
        assert!(c.dirty(), "wird gespeichert");
        assert!(!c.busy(), "nach dem Laden läuft nichts weiter");
    }

    #[test]
    fn tool_calls_and_context_start_are_recorded() {
        let b = EchoBackend { delay: Duration::ZERO };
        let meta = crate::chat::default_chat_meta();
        let mut c = ChatState::new(ChatFile::default(), None);
        for q in ["a", "b /tool /cut"] {
            c.draft = q.into();
            c.send(&b, &meta, &TurnContext::default(), Arc::new(|| {}));
            pump(&mut c);
        }
        assert_eq!(c.file.context_start, Some(2));
        assert_eq!(c.messages()[3].tool_calls[0].name, "list_sessions");
    }
}
