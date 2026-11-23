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
use super::context::ContextParts;
use super::logic;
use super::repair;
use super::store::{AllowRule, CallRepair, ChatFile, ChatMsg, Role, ToolCall, ToolStatus};
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
    /// Aufteilung der laufenden bzw. nächsten Anfrage für die Anzeige des Füllstands (#339): während einer Runde aus
    /// `TurnEvent::Context`, sonst von der App geschätzt (`llm_backend::context_estimate`). Nicht gespeichert.
    pub context: Option<ContextParts>,
    /// Stand (Verlauf, Einstellungen), für den die App `context` zuletzt geschätzt hat.
    pub context_key: Option<u64>,
    /// Offene Werkzeugaufrufe der laufenden Runde, in Reihenfolge (erledigte fallen weg).
    pub calls: Vec<PendingCall>,
    /// Nach den offenen Aufrufen folgt die nächste Anfrage an das Modell.
    next_request: bool,
    /// In dieser Nutzer-Runde verlangte Werkzeugaufrufe (ausgeführte und abgewiesene; zählen gegen `max_steps`).
    pub steps: u32,
    /// Wie oft `max_steps` freigegeben ist (1 + „Weiter“).
    grants: u32,
    /// Die Runde hielt an (Höchstzahl der Aufrufe oder Kontext voll; Knopf „Weiter“).
    pub paused: bool,
    /// Grund des Anhaltens, wenn es nicht die Höchstzahl war (z. B. „Werkzeug-Ergebnisse passen nicht mehr in den
    /// Kontext“); in der Sprache der Oberfläche.
    pub pause_note: Option<String>,
    /// Hat der aktuelle Schritt (Antwort mit Aufrufen) mindestens ein Werkzeug ausgeführt?
    step_executed: bool,
    /// Schritte dieser Runde in Folge, in denen kein einziger Aufruf ausgeführt wurde (nur abgewiesene).
    idle_steps: u32,
    /// Platz für den Verlauf laut Backend (geschätzte Tokens; Kontext ohne Antwort-Reserve, Systemprompt und
    /// Werkzeuge). `None` = unbekannt (dann gilt nur [`tools::MAX_RESULT_BYTES`]).
    pub history_budget: Option<usize>,
    /// Faktor, mit dem das Backend `history_budget` gerechnet hat (der damalige `token_scale`).
    budget_scale: f32,
    /// Geschätzte Tokens der letzten Anfrage (aus `TurnEvent::Budget`), zum Abgleich mit `prompt_tokens`.
    prompt_estimate: Option<usize>,
    /// So viel mehr Tokens zählt der Server als geschätzt (`prompt_tokens / Schätzung`, 1,0–4,0); geht als
    /// `TurnContext::token_scale` in die nächste Anfrage. Nicht gespeichert.
    pub token_scale: f32,
    /// Angehalten, weil kein Platz mehr im Kontext war: „Weiter“ startet gleich die nächste Anfrage (mit neu
    /// berechnetem Platz), statt wartende Aufrufe auszuführen.
    context_pause: bool,
    /// Ziel des wartenden Aufrufs für die Freigabe-Zeile, z. B. `session 4 "build"` (setzt der Hub).
    pub approval_target: Option<String>,
    /// Diff-Vorschau des wartenden `edit_file`/`write_file` (#338): (Kennung des Aufrufs, Vorschau); setzt der Hub
    /// zusammen mit `approval_target`.
    pub approval_preview: Option<(String, super::agent::files::Preview)>,
    /// Einmalige Frage nach der Stufe des Modells ist offen (setzt der Hub beim ersten Senden; siehe
    /// [`ChatState::awaiting_level_choice`]) – und wer sie ausgelöst hat: Nur bei [`LevelAsker::User`] geht nach der
    /// Antwort der Entwurf im Eingabefeld los.
    pub level_question: Option<LevelAsker>,
    /// Hat ein Agent (MCP `send_chat_message`) diese Runde ausgelöst: die Sitzungen der auslösenden Kette (der
    /// Aufrufer und – ist er selbst ein Chat – dessen Kette; leer = Nutzer oder Agent außerhalb von Paneldeck). Gilt für
    /// die ganze Runde samt „Weiter“; Werkzeuge des Chats dürfen in keine davon schreiben (kein Umweg in die Sitzung
    /// des Agenten, auch nicht über weitere Chats). Setzt `App::chat_send` (`tools::chain_origins`).
    pub origin_sessions: Vec<u64>,
    /// Der Hinweis „Kontext zu klein für die Werkzeug-Stufe“ wurde in diesem Chat schon gezeigt (einmal je Chat).
    pub context_warned: bool,
    /// Die laufende Antwort hat bisher nur Denk-Ausgabe geliefert (`TurnEvent::Thinking`; #299).
    pub thinking: bool,
    /// Beginn der laufenden Anfrage (für „Warte auf das Modell … N s“ solange nichts ankommt; #299).
    pub turn_started: Option<std::time::Instant>,
    /// Die eben fertige Antwort hat Text, aber keine Werkzeugaufrufe: Der nächste [`ChatState::tool_step`] sucht darin
    /// als Text geschriebene Aufrufe (#340; dafür braucht es die Werkzeug-Einstellungen, die erst dort vorliegen).
    text_check: bool,
    /// Letzter fehlgeschlagener Aufruf (Werkzeug, normalisierte Argumente) und wie oft er in Folge fehlschlug (#340).
    fail_streak: Option<(String, serde_json::Value)>,
    fail_count: u32,
    /// Die Runde hielt an, weil derselbe Aufruf [`LOOP_LIMIT`]-mal in Folge fehlschlug (deutlich angezeigt).
    pub loop_stop: bool,
    /// Offene Fragen des Modells an den Nutzer (`ask_user`, #356): das Formular; der Aufruf läuft, bis der Nutzer
    /// absendet oder abbricht. Nicht gespeichert (nach einem Neustart ist der Aufruf „unterbrochen“).
    pub ask: Option<super::ask::AskForm>,
    /// Nachrichten von Paneldeck (#355, z. B. „Hintergrund-Befehl beendet“), die noch nicht in den Verlauf durften,
    /// weil gerade eine Antwort läuft oder Werkzeugaufrufe offen sind ([`ChatState::push_event`]).
    events: Vec<PendingEvent>,
    /// Eine Nachricht von Paneldeck kam während der Runde und steht schon im Verlauf: der nächste `tool_step` fragt das
    /// Modell noch einmal an (geht nicht verloren, wenn sie außerhalb von `tool_step` einsortiert wurde).
    events_resume: bool,
}

/// Wartende Nachricht von Paneldeck.
struct PendingEvent {
    msg: ChatMsg,
    /// Kam sie, während die Runde lief? Dann macht das Modell danach weiter.
    resume: bool,
}

/// So oft in Folge darf derselbe fehlgeschlagene Aufruf (gleiches Werkzeug, gleiche Argumente) kommen, dann hält die
/// Runde an (#340).
pub const LOOP_LIMIT: u32 = 3;

/// Wer die einmalige Frage nach der Stufe des Modells ausgelöst hat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LevelAsker {
    /// Der Nutzer (Eingabefeld): nach der Antwort geht sein Entwurf los.
    User,
    /// Ein Agent (MCP `send_chat_message`, bekam `permission_pending`): nach der Antwort wird nichts gesendet – der
    /// Entwurf im Eingabefeld gehört dem Nutzer.
    Agent,
}

/// Status eines Chats für Seitenleiste und MCP (`ready`, `working`, `approval`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatStatus {
    Ready,
    Working,
    Approval,
}

/// Kennung für einen Werkzeugaufruf der Antwort `i`, die im ganzen Chat und unter `pending` (Aufrufe, die gerade erst
/// angelegt werden) noch frei ist: `call_<i>_<k>`, `k` ab `from`. Ollama vergibt je Antwort `call_0`, `call_1` …, darum
/// reichen die Kennungen des Servers nicht.
fn unique_call_id(msgs: &[ChatMsg], pending: &[ToolCall], i: usize, from: usize) -> String {
    let taken = |id: &str| msgs.iter().any(|m| m.tool_calls.iter().any(|x| x.id == id)) || pending.iter().any(|x| x.id == id);
    let mut k = from;
    loop {
        let id = format!("call_{i}_{k}");
        if !taken(&id) {
            return id;
        }
        k += 1;
    }
}

/// Kleinster sinnvoller Platz für ein neues Ergebnis (Bytes); weniger → ältere Ergebnisse der Runde entfernen bzw.
/// anhalten.
pub const MIN_RESULT_BYTES: usize = 1024;

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
            context: None,
            context_key: None,
            calls: Vec::new(),
            next_request: false,
            steps: 0,
            grants: 1,
            paused: false,
            pause_note: None,
            step_executed: false,
            idle_steps: 0,
            history_budget: None,
            budget_scale: 1.0,
            prompt_estimate: None,
            token_scale: 1.0,
            context_pause: false,
            approval_target: None,
            approval_preview: None,
            level_question: None,
            origin_sessions: Vec::new(),
            context_warned: false,
            thinking: false,
            turn_started: None,
            text_check: false,
            fail_streak: None,
            fail_count: 0,
            loop_stop: false,
            ask: None,
            events: Vec::new(),
            events_resume: false,
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
        if !self.streaming() && self.calls.is_empty() && self.events.is_empty() {
            return std::borrow::Cow::Borrowed(&self.file);
        }
        let mut f = self.file.clone();
        if self.streaming() {
            if let Some(m) = f.messages.get_mut(self.turn_msg) {
                m.error = Some(state::t("unterbrochen", "interrupted").to_string());
            }
        }
        logic::close_orphans(&mut f.messages, tools::interrupted);
        // Wartende Nachrichten von Paneldeck (#355, z. B. während die Runde angehalten ist) gehen nicht verloren: in der
        // Datei stehen sie hinter den (dort als unterbrochen geschlossenen) Aufrufen.
        f.messages.extend(self.events.iter().map(|e| e.msg.clone()));
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
        self.streaming() || (!self.paused && (!self.calls.is_empty() || self.next_request || self.text_check))
    }

    /// Ein schreibender Aufruf wartet auf die Entscheidung des Nutzers.
    pub fn awaiting_approval(&self) -> bool {
        self.calls.iter().any(|c| c.state == CallState::Approval)
    }

    /// Die einmalige Frage „Darf dieses Modell Paneldeck nutzen?“ wartet auf eine Antwort (die Nachricht im
    /// Eingabefeld geht erst danach los). Zählt wie eine Freigabe als „braucht dich“.
    pub fn awaiting_level_choice(&self) -> bool {
        self.level_question.is_some()
    }

    /// Das Modell hat dem Nutzer Fragen gestellt (`ask_user`, #356), die noch offen sind. Zählt wie eine Freigabe als
    /// „braucht dich“ (Status `approval`, Hinweis wie bei Freigaben); in der Seitenleiste „wartet auf dich“.
    pub fn awaiting_answer(&self) -> bool {
        self.ask.as_ref().is_some_and(|f| self.calls.iter().any(|c| c.state == CallState::Running && c.call.id == f.call_id))
    }

    /// Fragen des Modells zeigen (`ask_user`): das Formular gehört zum gerade laufenden Aufruf `form.call_id`.
    pub fn open_ask(&mut self, form: super::ask::AskForm) {
        self.ask = Some(form);
        self.follow = true;
        self.touch();
    }

    /// Nur für die Doku-Bilder (`readme-shots`): ein laufender `ask_user`-Aufruf mit offenem Formular, ohne Hub.
    #[cfg(feature = "readme-shots")]
    pub fn shot_ask(&mut self, call: ToolCall, form: super::ask::AskForm) {
        self.calls.push(PendingCall { call, state: CallState::Running, rx: None });
        self.open_ask(form);
    }

    /// Antwort des Nutzers auf die offenen Fragen (Absenden oder Abbrechen): das Ergebnis geht an den wartenden Aufruf.
    /// `false`, wenn es keine Fragen gibt oder „Absenden“ noch nicht geht (nicht alles beantwortet).
    pub fn answer_ask(&mut self, r: super::ask::AskReply) -> bool {
        let ok = self.ask.as_mut().is_some_and(|f| f.reply(r));
        if ok {
            self.ask = None;
            self.focus_input = true;
            self.touch();
        }
        ok
    }

    /// Nachricht von Paneldeck in den Chat (#355: ein Hintergrund-Befehl ist zu Ende). `text` ist für das Modell (geht als
    /// Nachricht des Nutzers mit), `event` für die Anzeige. Läuft gerade eine Antwort oder sind Werkzeugaufrufe offen,
    /// wartet sie, bis der Verlauf wieder stimmig ist (ein Ergebnis muss direkt auf seinen Aufruf folgen). Kam sie während
    /// einer laufenden Runde, macht das Modell danach weiter – auch wenn es seine Antwort schon ohne Aufrufe beendet hat.
    pub fn push_event(&mut self, event: super::store::ChatEvent, text: String) {
        let mut msg = ChatMsg::new(Role::System, text);
        msg.event = Some(event);
        let resume = self.busy();
        self.events.push(PendingEvent { msg, resume });
        self.flush_events();
        self.touch();
    }

    /// Warten Nachrichten von Paneldeck auf ihren Platz im Verlauf?
    pub fn events_pending(&self) -> bool {
        !self.events.is_empty() || self.events_resume
    }

    /// Wartende Nachrichten in den Verlauf, wenn es geht (keine Antwort läuft, keine offenen Aufrufe, keine Antwort
    /// wartet auf die Suche nach Text-Aufrufen). `true`, wenn eine davon während der Runde kam (dann weiter anfragen).
    fn flush_events(&mut self) -> bool {
        if self.events.is_empty() || self.streaming() || !self.calls.is_empty() || self.text_check {
            return false;
        }
        let at = self.file.messages.len();
        let mut resume = false;
        for e in std::mem::take(&mut self.events) {
            resume |= e.resume;
            self.file.messages.push(e.msg);
        }
        self.follow = true;
        self.changed(at);
        self.events_resume |= resume;
        resume
    }

    /// Status für Seitenleiste und MCP: Freigabe/Frage offen → `Approval`, Runde läuft → `Working`, sonst `Ready`.
    pub fn status(&self) -> ChatStatus {
        if self.awaiting_approval() || self.awaiting_level_choice() || self.awaiting_answer() {
            ChatStatus::Approval
        } else if self.busy() {
            ChatStatus::Working
        } else {
            ChatStatus::Ready
        }
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

    /// Verlauf leeren (Aufräumen, #307): der Chat wird leer, Eingabe und dauerhafte Freigaben bleiben. Nicht während einer
    /// Runde oder Bearbeitung (`false`).
    pub fn clear_history(&mut self) -> bool {
        if self.busy() || self.editing {
            return false;
        }
        self.file.messages.clear();
        // Die To-do-Liste gehört zum Gespräch (#338).
        self.file.todos.clear();
        self.ui = UiCache::default();
        self.usage = None;
        self.context = None;
        self.context_key = None;
        self.notice = None;
        self.steps = 0;
        self.paused = false;
        self.pause_note = None;
        self.context_pause = false;
        self.text_check = false;
        self.loop_stop = false;
        self.changed(0);
        true
    }

    /// Ungespeicherte Änderungen?
    pub fn dirty(&self) -> bool {
        self.saved.get() != self.rev
    }

    pub fn mark_saved(&self) {
        self.saved.set(self.rev);
    }

    /// Diesen Text als neue Nachricht senden (externer Agent): Entwurf und Bearbeiten im Eingabefeld bleiben unberührt.
    /// `false`, wenn nichts zu senden ist oder die Runde noch läuft.
    pub fn send_text(&mut self, text: &str, backend: &dyn ChatBackend, meta: &ChatMeta, cx: &TurnContext, repaint: Repaint) -> bool {
        let draft = std::mem::replace(&mut self.draft, text.to_string());
        let editing = std::mem::replace(&mut self.editing, false);
        let before = self.draft_before_edit.take();
        let sent = self.send(backend, meta, cx, repaint);
        self.draft = draft;
        self.editing = editing;
        self.draft_before_edit = before;
        sent
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
        self.pause_note = None;
        self.context_pause = false;
        self.idle_steps = 0;
        self.step_executed = false;
        self.fail_streak = None;
        self.fail_count = 0;
        self.loop_stop = false;
    }

    fn start_turn(&mut self, backend: &dyn ChatBackend, meta: &ChatMeta, cx: &TurnContext, repaint: Repaint) {
        // Die Kürzung gilt nur für eine Anfrage; das Backend meldet sie neu (`TurnEvent::ContextStart`), wenn es kürzt.
        self.file.context_start = None;
        self.usage = None;
        self.thinking = false;
        self.text_check = false;
        self.turn_started = Some(std::time::Instant::now());
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
        self.text_check = false;
        self.paused = false;
        self.pause_note = None;
        self.context_pause = false;
        self.loop_stop = false;
        self.approval_target = None;
        self.approval_preview = None;
        // Offene Fragen (#356) gehören zur Runde; ihr Aufruf hat jetzt „abgebrochen“.
        self.ask = None;
        if had || fixed > 0 {
            self.changed(self.turn_msg.min(self.file.messages.len()));
        }
        // Wartende Nachrichten von Paneldeck (#355) kommen jetzt in den Verlauf (die Runde ist vorbei, keine neue Anfrage).
        self.flush_events();
        self.events_resume = false;
    }

    /// Ergebnis des ersten offenen Aufrufs eintragen und ihn entfernen.
    fn finish_first(&mut self, status: ToolStatus, content: String) {
        if self.calls.is_empty() {
            return;
        }
        let p = self.calls.remove(0);
        let at = self.file.messages.len();
        self.file.messages.push(logic::tool_result(&p.call, status, content));
        self.approval_target = None;
        self.approval_preview = None;
        if self.ask.as_ref().is_some_and(|f| f.call_id == p.call.id) {
            self.ask = None;
        }
        self.changed(at);
    }

    fn pause(&mut self, note: Option<String>) {
        self.paused = true;
        self.pause_note = note;
        self.touch();
    }

    /// Anhalten, weil der Kontext voll ist („Weiter“ startet dann gleich die nächste Anfrage).
    fn pause_context(&mut self) {
        self.context_pause = true;
        self.pause(Some(
            state::t(
                "Die Werkzeug-Ergebnisse passen nicht mehr in den Kontext des Modells – Kontextlänge in Einstellungen → Modelle erhöhen.",
                "The tool results no longer fit into the model's context – raise the context length in Settings → Models.",
            )
            .to_string(),
        ));
    }

    /// Erste Nachricht der laufenden Runde (nach der letzten Nutzer-Nachricht).
    fn round_start(&self) -> usize {
        logic::last_user(&self.file.messages).map_or(0, |i| i + 1)
    }

    /// Größe der laufenden Runde in geschätzten Tokens (wie das Backend schätzt).
    fn round_tokens(&self) -> usize {
        self.file.messages[logic::last_user(&self.file.messages).unwrap_or(0)..].iter().filter(|m| logic::sendable(m)).map(logic::msg_tokens).sum()
    }

    /// Wie groß ein neues Werkzeug-Ergebnis höchstens sein darf (Bytes): [`tools::MAX_RESULT_BYTES`], bei bekanntem
    /// Platz höchstens die Hälfte dessen, was in der Runde noch frei ist. Der Platz ist in geschätzten Tokens (ASCII ≈
    /// 3 Zeichen je Token) und wird mit dem neuesten Abgleich `token_scale` nachkorrigiert.
    pub fn result_cap(&self) -> usize {
        self.result_cap_with(tools::MAX_RESULT_BYTES)
    }

    /// Wie [`ChatState::result_cap`], aber mit der Grenze des Werkzeugs `tool` (`read_file`: 32 KB, #338).
    pub fn result_cap_for(&self, tool: &str) -> usize {
        self.result_cap_with(tools::max_result_bytes(tool))
    }

    fn result_cap_with(&self, limit: usize) -> usize {
        match self.history_budget {
            None => limit,
            Some(b) => {
                let b = (b as f64 * self.budget_scale as f64 / self.token_scale.max(1.0) as f64) as usize;
                (b.saturating_sub(self.round_tokens()) * 3 / 2).min(limit)
            }
        }
    }

    /// Ergebnisse der Runde, die das Modell schon gesehen hat (vor der laufenden Antwort), durch einen kurzen
    /// Platzhalter ersetzen. Ergebnisse des aktuellen Schritts bleiben – die kennt das Modell noch nicht. `true`, wenn
    /// etwas frei wurde.
    fn elide_round_results(&mut self) -> bool {
        let from = self.round_start();
        let to = self.turn_msg.min(self.file.messages.len());
        let short = tools::elided();
        let mut any = false;
        for i in from..to {
            let m = &mut self.file.messages[i];
            if m.role == Role::Tool && m.content.len() > short.len() {
                m.content = short.clone();
                any = true;
                self.changed_from = self.changed_from.min(i);
            }
        }
        if any {
            self.touch();
        }
        any
    }

    /// Ist Platz für ein weiteres Ergebnis? Wenn nicht, werden zuerst schon gesehene Ergebnisse der Runde entfernt.
    fn room_for_result(&mut self) -> bool {
        if self.result_cap() >= MIN_RESULT_BYTES {
            return true;
        }
        self.elide_round_results() && self.result_cap() >= MIN_RESULT_BYTES
    }

    /// Ergebnis eines ausgeführten Aufrufs eintragen – so gekürzt, dass die Runde in den Kontext passt. (Vor der
    /// Ausführung wurde schon geprüft, dass Platz ist; reicht er trotzdem nicht, kommt ein Platzhalter und die Runde
    /// hält an – der Systemprompt darf nie abgeschnitten werden.)
    fn record_result(&mut self, name: &str, r: ToolResult) {
        let call = self.calls.first().map(|p| p.call.clone());
        let failed = r.is_err();
        if !self.room_for_result() {
            let status = if r.is_ok() { ToolStatus::Ok } else { ToolStatus::Error };
            self.finish_first(status, tools::elided());
            self.pause_context();
            return;
        }
        let (st, content) = tools::result_message(name, r, self.result_cap_for(name));
        self.finish_first(st, content);
        if let Some(c) = call {
            if self.note_outcome(&c, failed) {
                self.stop_loop(&c.name);
            }
        }
    }

    /// Ausgang eines Aufrufs für den Schleifen-Schutz merken. `true`, wenn derselbe Aufruf (gleiches Werkzeug, gleiche
    /// normalisierte Argumente) jetzt [`LOOP_LIMIT`]-mal in Folge fehlschlug. Ein gelungener oder anderer Aufruf
    /// beginnt die Zählung neu.
    fn note_outcome(&mut self, call: &ToolCall, failed: bool) -> bool {
        if !failed {
            self.fail_streak = None;
            self.fail_count = 0;
            return false;
        }
        let key = (call.name.clone(), tools::exec_args(call));
        if self.fail_streak.as_ref() == Some(&key) {
            self.fail_count += 1;
        } else {
            self.fail_streak = Some(key);
            self.fail_count = 1;
        }
        self.fail_count >= LOOP_LIMIT
    }

    /// Schleifen-Schutz: anhalten, deutlich sichtbar („Weiter“ gibt die Runde wieder frei; kommt derselbe Fehler danach
    /// noch einmal, hält sie gleich wieder an).
    fn stop_loop(&mut self, tool: &str) {
        let tool = crate::mcp::short(tool, 60);
        let note = if state::lang() == state::Lang::De {
            format!("Das Modell hat {LOOP_LIMIT}-mal hintereinander denselben fehlerhaften Werkzeugaufruf ({tool}) gesendet – angehalten.")
        } else {
            format!("The model sent the same failing tool call ({tool}) {LOOP_LIMIT} times in a row – stopped.")
        };
        self.loop_stop = true;
        self.pause(Some(note));
    }

    /// Als Text geschriebene Werkzeugaufrufe der eben fertigen Antwort übernehmen (#340): Sie kommen wie echte Aufrufe
    /// in die Antwort (der Text des Aufrufs fällt aus dem Antworttext) und laufen danach durch dieselbe Prüfung,
    /// Freigabe und Ausführung. Nur wenn das Modell überhaupt Werkzeuge hat. `true`, wenn welche erkannt wurden.
    fn adopt_text_calls(&mut self, policy: &ToolPolicy) -> bool {
        let i = self.turn_msg;
        if !policy.enabled() || i + 1 != self.file.messages.len() {
            return false;
        }
        let m = &self.file.messages[i];
        if m.role != Role::Assistant || m.error.is_some() || !m.tool_calls.is_empty() {
            return false;
        }
        let Some(found) = repair::find_text_calls(&m.content, &|n| crate::mcp::tool_level(n).is_some() || super::agent::is_agent_tool(n)) else { return false };
        let mut calls = Vec::new();
        let mut repairs = Vec::new();
        for (k, tc) in found.calls.into_iter().enumerate() {
            let mut c = ToolCall { id: String::new(), name: tc.name, arguments: tc.arguments };
            c.id = unique_call_id(&self.file.messages, &calls, i, k);
            let fixes = repair::normalize(&mut c);
            repairs.push(CallRepair { id: c.id.clone(), text_call: true, fixes });
            calls.push(c);
        }
        let m = &mut self.file.messages[i];
        m.content = found.rest;
        m.tool_calls = calls;
        m.repairs = repairs;
        self.calls = m.tool_calls.iter().map(|c| PendingCall { call: c.clone(), state: CallState::Queued, rx: None }).collect();
        self.next_request = true;
        self.changed(i);
        true
    }

    /// Nächster Schritt der Werkzeug-Schleife (im Hauptdurchgang, jedes Bild). Ungültige oder verbotene Aufrufe
    /// bekommen hier gleich ihr Fehler-Ergebnis; die Einstellungen werden bei jedem Aufruf neu geprüft.
    ///
    /// * Jeder verlangte Aufruf zählt gegen `max_steps` (auch abgewiesene, sonst liefe ein Modell, das immer wieder
    ///   Unsinn aufruft, endlos).
    /// * Schritte in Folge, die nichts ausführen konnten, halten die Runde nach `max(max_steps, 2)` an (eigener
    ///   Hinweis; bei `max_steps = 1` sieht das Modell so die Ablehnung noch einmal).
    /// * Kein Aufruf läuft ohne Platz für sein Ergebnis: dann hält die Runde vorher an.
    pub fn tool_step(&mut self, policy: &ToolPolicy) -> ToolStep {
        if self.streaming() || self.paused {
            return ToolStep::Idle;
        }
        if std::mem::take(&mut self.text_check) {
            self.adopt_text_calls(policy);
        }
        let max = policy.cfg.max_steps.max(1);
        loop {
            let Some(p) = self.calls.first() else {
                // Nachrichten von Paneldeck (#355) kommen vor die nächste Anfrage; kam eine während der Runde, fragt der
                // Chat das Modell danach noch einmal (auch wenn die Antwort schon ohne Aufrufe endete).
                // Nach einer fehlgeschlagenen Antwort nicht von selbst weiter (der Server ist vielleicht weg).
                let errored = self.has_error();
                self.flush_events();
                let resume = std::mem::take(&mut self.events_resume) && !errored;
                if !self.next_request {
                    return if resume { ToolStep::Request } else { ToolStep::Idle };
                }
                // Schritt zu Ende: nichts ausgeführt → zählt als leerer Schritt.
                if std::mem::take(&mut self.step_executed) {
                    self.idle_steps = 0;
                } else {
                    self.idle_steps += 1;
                }
                if self.idle_steps >= max.max(2) {
                    self.pause(Some(
                        state::t(
                            "Das Modell rief immer wieder Werkzeuge auf, die nicht laufen konnten – angehalten.",
                            "The model kept calling tools that could not run – stopped.",
                        )
                        .to_string(),
                    ));
                    return ToolStep::Idle;
                }
                self.next_request = false;
                return ToolStep::Request;
            };
            match p.state {
                CallState::Running | CallState::Approval => return ToolStep::Idle,
                CallState::Queued | CallState::Approved => {}
            }
            let approved = p.state == CallState::Approved;
            // Erster Blick auf diesen Aufruf: Höchstzahl (gezählt wird unten, auch wenn er abgewiesen wird).
            if !approved && self.steps >= max.saturating_mul(self.grants) {
                self.pause(None);
                return ToolStep::Idle;
            }
            let ask = match policy.check(&p.call) {
                Ok(w) => w,
                Err((st, content)) => {
                    if !approved {
                        self.steps += 1;
                    }
                    let call = p.call.clone();
                    self.finish_first(st, content);
                    if self.note_outcome(&call, true) {
                        self.stop_loop(&call.name);
                        return ToolStep::Idle;
                    }
                    continue;
                }
            };
            // Ohne Platz für das Ergebnis läuft nichts (und es wird auch nicht erst gefragt).
            if !self.room_for_result() {
                self.pause_context();
                return ToolStep::Idle;
            }
            if !approved {
                self.steps += 1;
                if policy.needs_approval(ask, &self.calls[0].call, &self.file.always_allow) {
                    self.calls[0].state = CallState::Approval;
                    self.touch();
                    return ToolStep::Idle;
                }
            }
            self.step_executed = true;
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
        self.record_result(&name, r);
        true
    }

    /// Der Aufruf, der gerade auf Freigabe wartet.
    pub fn pending_approval(&self) -> Option<&ToolCall> {
        self.calls.iter().find(|c| c.state == CallState::Approval).map(|c| &c.call)
    }

    /// Entscheidung des Nutzers zum wartenden Aufruf `call_id`. `false` (und nichts geschieht), wenn gerade kein oder
    /// ein anderer Aufruf wartet – z. B. ein veralteter Klick nach Stopp. „Immer“ gilt nur für Aufrufe mit
    /// Ziel-Sitzung ([`AllowRule::eligible`]); sonst wirkt es wie „Erlauben“.
    pub fn decide(&mut self, call_id: &str, d: Decision) -> bool {
        let Some(i) = self.calls.iter().position(|c| c.state == CallState::Approval && c.call.id == call_id) else { return false };
        match d {
            Decision::Allow => self.calls[i].state = CallState::Approved,
            Decision::Always => {
                if AllowRule::eligible(&self.calls[i].call) {
                    let rule = AllowRule::for_call(&self.calls[i].call);
                    if !self.file.always_allow.contains(&rule) {
                        self.file.always_allow.push(rule);
                    }
                }
                self.calls[i].state = CallState::Approved;
            }
            Decision::Deny => {
                // Nur der erste Aufruf kann warten (Reihenfolge), also ist `i == 0`.
                let (st, content) = tools::denied();
                self.finish_first(st, content);
            }
        }
        self.approval_target = None;
        self.approval_preview = None;
        self.touch();
        true
    }

    /// To-do-Liste des Chats ersetzen (`update_todos`, #338).
    pub fn set_todos(&mut self, todos: Vec<super::store::ChatTodo>) {
        if self.file.todos != todos {
            self.file.todos = todos;
            self.touch();
        }
    }

    /// Dauerhafte Freigabe zurücknehmen. `false`, wenn es sie nicht gab.
    pub fn revoke(&mut self, rule: &AllowRule) -> bool {
        let before = self.file.always_allow.len();
        self.file.always_allow.retain(|r| r != rule);
        let changed = self.file.always_allow.len() != before;
        if changed {
            self.touch();
        }
        changed
    }

    /// „Weiter“ nach dem Anhalten: noch einmal so viele Aufrufe freigeben. Nach „Kontext voll“ laufen die wartenden
    /// Aufrufe nicht mehr (sie bekommen „nicht ausgeführt“); stattdessen geht gleich die nächste Anfrage mit neu
    /// berechnetem Platz los – das Modell entscheidet dann selbst, was es noch braucht.
    pub fn resume(&mut self) -> bool {
        if !self.paused {
            return false;
        }
        if std::mem::take(&mut self.context_pause) {
            for p in std::mem::take(&mut self.calls) {
                let content = tools::error_json("skipped", "Not run: the model's context was full. Call it again if you still need it.");
                self.file.messages.push(logic::tool_result(&p.call, ToolStatus::Error, content));
            }
            self.next_request = true;
            self.history_budget = None;
            self.changed(self.turn_msg.min(self.file.messages.len()));
        }
        self.paused = false;
        self.pause_note = None;
        // Der Schleifen-Schutz zählt weiter: schlägt derselbe Aufruf noch einmal fehl, hält die Runde gleich wieder an.
        self.loop_stop = false;
        self.idle_steps = 0;
        self.step_executed = true; // „Weiter“ zählt nicht als leerer Schritt
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
                TurnEvent::Thinking => self.thinking = true,
                TurnEvent::ToolCall(mut c) => {
                    // Kennungen müssen im ganzen Chat eindeutig sein (Ollama vergibt je Antwort `call_0`, `call_1` …).
                    if c.id.is_empty() || self.file.messages.iter().any(|m| m.tool_calls.iter().any(|x| x.id == c.id)) {
                        let k = self.file.messages.get(i).map_or(0, |m| m.tool_calls.len());
                        c.id = unique_call_id(&self.file.messages, &[], i, k);
                    }
                    if let Some(m) = self.file.messages.get_mut(i) {
                        m.tool_calls.push(c);
                    }
                }
                TurnEvent::ContextStart(c) => self.file.context_start = logic::clamp_context_start(Some(c), i),
                TurnEvent::Usage { prompt_tokens, completion_tokens } => {
                    self.usage = Some((prompt_tokens, completion_tokens));
                    // Abgleich: so viel mehr zählt der Server als geschätzt (nie weniger als die Schätzung annehmen).
                    if let (Some(real), Some(est)) = (prompt_tokens, self.prompt_estimate.filter(|e| *e > 0)) {
                        self.token_scale = (real as f32 / est as f32).clamp(1.0, 4.0);
                    }
                }
                TurnEvent::Budget { history, prompt } => {
                    self.history_budget = Some(history);
                    self.budget_scale = self.token_scale;
                    self.prompt_estimate = Some(prompt);
                }
                TurnEvent::Context(parts) => self.context = Some(parts),
                TurnEvent::ContextNotice(n) => {
                    if !self.context_warned {
                        self.context_warned = true;
                        if let Some(m) = self.file.messages.get_mut(i) {
                            m.note = Some(match m.note.take() {
                                Some(old) => format!("{old}\n{n}"),
                                None => n,
                            });
                        }
                    }
                }
                TurnEvent::Note(n) => {
                    if let Some(m) = self.file.messages.get_mut(i) {
                        m.note = Some(n);
                    }
                }
                TurnEvent::Done => {
                    self.turn = None;
                    p.finished = true;
                    // Werkzeugaufrufe: die Runde geht weiter (der Hub führt sie aus, dann folgt die nächste Anfrage). Ihre
                    // Argumente werden vorher gegen das Schema normalisiert (#340; gespeichert wird die korrigierte Fassung,
                    // die Korrekturen stehen in `repairs`). Nur Text: vielleicht ein als Text geschriebener Aufruf – das
                    // prüft der nächste `tool_step`.
                    if let Some(m) = self.file.messages.get_mut(i).filter(|m| m.error.is_none()) {
                        if !m.tool_calls.is_empty() {
                            for c in &mut m.tool_calls {
                                let fixes = repair::normalize(c);
                                if !fixes.is_empty() {
                                    m.repairs.push(CallRepair { id: c.id.clone(), text_call: false, fixes });
                                }
                            }
                            self.calls = m.tool_calls.iter().map(|c| PendingCall { call: c.clone(), state: CallState::Queued, rx: None }).collect();
                            self.next_request = true;
                        } else if repair::may_contain_call(&m.content) {
                            self.text_check = true;
                        }
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

    /// Füllstand (#339): die Aufteilung kommt mit der Anfrage, die Zählung des Servers mit der Antwort; Leeren setzt
    /// beides zurück.
    #[test]
    fn context_parts_and_server_count_feed_the_fill_level() {
        let parts = ContextParts { system: 3000, tools: 1000, history: 300, max: 8192, reserve: 2048, dropped: false, scale: 1.0 };
        let b = Script::new(vec![vec![TurnEvent::Context(parts), TurnEvent::Delta("a".into()), TurnEvent::Usage { prompt_tokens: Some(4000), completion_tokens: Some(342) }, TurnEvent::Done]]);
        let mut c = ChatState::new(ChatFile::default(), None);
        c.context_key = Some(7);
        start(&mut c, &b, "q");
        assert!(c.poll().finished);
        assert_eq!(c.context, Some(parts));
        let f = super::super::context::fill(c.context.as_ref(), c.usage, c.file.context_start.is_some()).unwrap();
        assert_eq!(f.text(), "4342/8192 (53%)");
        assert!(c.clear_history());
        assert_eq!((c.context, c.context_key, c.usage), (None, None, None));
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
        ToolPolicy::new(crate::llm::config::LlmModel { level, max_steps, level_chosen: true, ..crate::llm::config::LlmModel::named("m") }, None, false)
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
        assert_eq!(c.steps, 3, "auch abgewiesene Aufrufe zählen (sonst Endlosschleife)");
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
        // (gelingende Aufrufe: derselbe fehlschlagende Aufruf hielte schon der Schleifen-Schutz an, siehe unten)
        assert_eq!(drive(&mut c, &b, &p, &ok_exec).len(), 3);
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
        // Ablehnen (eine veraltete Kennung bewirkt nichts)
        assert!(!c.decide("stale", Decision::Deny));
        assert!(c.awaiting_approval());
        assert!(c.decide("w", Decision::Deny));
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
        let id = c.pending_approval().unwrap().id.clone();
        assert!(c.decide(&id, Decision::Always));
        assert_eq!(drive(&mut c, &b, &p, &ok_exec), ["send_text"]);
        let rule = AllowRule { tool: "send_text".into(), session: Some(2), prefix: None, ..AllowRule::default() };
        assert_eq!(c.file.always_allow, [rule.clone()]);
        c.draft = "third".into();
        c.send(&b, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {}));
        assert_eq!(drive(&mut c, &b, &p, &ok_exec), ["send_text"], "gleiches Werkzeug, gleiches Ziel: ohne Nachfrage");
        assert!(!c.awaiting_approval());
        // Gespeichert/geladen bleibt „immer“ erhalten
        let back = super::super::store::parse(std::str::from_utf8(&super::super::store::serialize_capped(&c.file, 1 << 20)).unwrap()).unwrap().0;
        assert_eq!(back.always_allow, [rule.clone()]);
        // Anderes Ziel: wieder fragen
        let other = Script::new(vec![vec![tc("o", "send_text", serde_json::json!({"session": 3, "text": "dir"})), TurnEvent::Done], vec![TurnEvent::Delta("c".into()), TurnEvent::Done]]);
        c.draft = "fourth".into();
        c.send(&other, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {}));
        drive(&mut c, &other, &p, &|_| panic!("andere Sitzung: erst nach Freigabe"));
        assert!(c.awaiting_approval());
        c.stop();
        // Widerrufen: wieder fragen
        assert!(c.revoke(&rule) && !c.revoke(&rule));
        assert!(c.file.always_allow.is_empty());
        c.draft = "fifth".into();
        let b = Script::new(vec![vec![call(), TurnEvent::Done], vec![TurnEvent::Delta("e".into()), TurnEvent::Done]]);
        c.send(&b, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {}));
        drive(&mut c, &b, &p, &|_| panic!("widerrufen: erst nach Freigabe"));
        assert!(c.awaiting_approval());
    }

    #[test]
    fn calls_that_never_execute_stop_the_round() {
        // Jede Antwort verlangt ein verbotenes Werkzeug (immer mit anderem Text, sonst hielte schon der Schleifen-Schutz
        // an): zählt, und nach `max_steps` ist Schluss.
        let turn = |t: &str| vec![tc("x", "send_text", serde_json::json!({"session": 1, "text": t})), TurnEvent::Done];
        let b = Script::new(vec![turn("a"), turn("b"), turn("a"), turn("b"), turn("a"), turn("b"), turn("a"), turn("b"), turn("a"), turn("b")]);
        let p = policy(crate::mcp::McpLevel::Read, 5);
        let mut c = ChatState::new(ChatFile::default(), None);
        c.draft = "go".into();
        c.send(&b, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {}));
        assert!(drive(&mut c, &b, &p, &|_| panic!("verboten")).is_empty());
        assert!(c.paused && !c.busy());
        assert!(c.steps <= 5);
        consistent(&c);
        // Leere Schritte ohne Aufrufzählung (z. B. Höchstzahl hoch, aber nie etwas ausgeführt): eigene Grenze
        let mut c = ChatState::new(ChatFile::default(), None);
        c.draft = "go".into();
        c.send(&b, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {}));
        c.grants = 1000; // Aufrufgrenze praktisch aus
        drive(&mut c, &b, &p, &|_| panic!("verboten"));
        assert!(c.paused, "nach {} leeren Schritten angehalten", p.cfg.max_steps);
        assert_eq!(c.pause_note.as_deref(), Some("The model kept calling tools that could not run – stopped."), "eigener Hinweis");
        let requests = c.messages().iter().filter(|m| m.role == Role::Assistant).count();
        assert_eq!(requests, 5);
    }

    #[test]
    fn results_shrink_to_the_context_and_the_round_pauses_when_full() {
        let big = |_: &ToolCall| -> ToolResult { Ok(serde_json::json!({"text": "x".repeat(50_000)})) };
        let b = Script::new(vec![vec![TurnEvent::Budget { history: 3_000, prompt: 100 }, tc("a", "read_terminal", serde_json::json!({"session": 1})), TurnEvent::Done]]);
        let p = policy(crate::mcp::McpLevel::Read, 10);
        let mut c = ChatState::new(ChatFile::default(), None);
        c.draft = "read".into();
        c.send(&b, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {}));
        assert_eq!(c.result_cap(), tools::MAX_RESULT_BYTES, "ohne bekannten Platz: Höchstgrenze");
        drive(&mut c, &b, &p, &big);
        assert_eq!(c.history_budget, Some(3_000));
        let tools_msgs: Vec<&ChatMsg> = c.messages().iter().filter(|m| m.role == Role::Tool).collect();
        assert_eq!(tools_msgs.len(), 10);
        // Jedes Ergebnis: höchstens die Hälfte des freien Platzes (am Anfang ≈ 6000 Bytes), nie 16 KiB; das Ende bleibt
        assert!(tools_msgs.iter().all(|m| m.content.len() <= 6_000), "{:?}", tools_msgs.iter().map(|m| m.content.len()).collect::<Vec<_>>());
        assert!(tools_msgs.last().unwrap().content.contains("truncated_front"));
        // Wurde es eng, wurden ältere Ergebnisse der Runde entfernt – die Runde blieb immer im Platz
        assert!(tools_msgs.iter().any(|m| m.content.contains("\"elided\"")));
        assert!(c.round_tokens() <= 3_000, "die Runde bleibt im Platz: {}", c.round_tokens());
        assert!(c.paused && c.pause_note.is_none(), "hier hielt die Höchstzahl an");
        let mut saved = c.file_for_save().into_owned().messages;
        assert_eq!(logic::close_orphans(&mut saved, tools::interrupted), 0, "gespeichert stimmig (der wartende Aufruf als unterbrochen)");
        // Viel zu wenig Platz: das Werkzeug läuft gar nicht erst, die Runde hält mit Hinweis an
        let tiny = Script::new(vec![vec![TurnEvent::Budget { history: 200, prompt: 100 }, tc("a", "read_terminal", serde_json::json!({"session": 1})), TurnEvent::Done]]);
        let mut c = ChatState::new(ChatFile::default(), None);
        c.draft = "read".into();
        c.send(&tiny, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {}));
        assert!(drive(&mut c, &tiny, &p, &|_| panic!("ohne Platz läuft nichts")).is_empty());
        assert!(c.paused && !c.busy());
        assert_eq!(c.pause_note.as_deref(), Some("The tool results no longer fit into the model's context – raise the context length in Settings → Models."));
        assert_eq!(c.steps, 0, "nicht ausgeführt, nicht gezählt");
        let mut saved = c.file_for_save().into_owned().messages;
        assert_eq!(logic::close_orphans(&mut saved, tools::interrupted), 0);
        // „Weiter“: der wartende Aufruf bekommt „nicht ausgeführt“, gleich folgt die nächste Anfrage (Platz neu berechnet)
        assert!(c.resume() && c.pause_note.is_none());
        assert!(c.calls.is_empty());
        assert!(c.messages().last().unwrap().content.contains("\"skipped\""));
        consistent(&c);
        assert_eq!(c.tool_step(&p), ToolStep::Request);
    }

    #[test]
    fn eliding_never_touches_results_the_model_has_not_seen() {
        // Zwei Aufrufe in einem Schritt; das erste Ergebnis ist groß. Vor dem zweiten fehlt Platz – das erste Ergebnis
        // derselben Antwort bleibt trotzdem (das Modell kennt es noch nicht); stattdessen hält die Runde an.
        let b = Script::new(vec![vec![TurnEvent::Budget { history: 1_200, prompt: 100 }, tc("a", "read_terminal", serde_json::json!({"session": 1})), tc("b", "read_terminal", serde_json::json!({"session": 2})), TurnEvent::Done]]);
        let p = policy(crate::mcp::McpLevel::Read, 10);
        let mut c = ChatState::new(ChatFile::default(), None);
        c.draft = "read".into();
        c.send(&b, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {}));
        let ran = drive(&mut c, &b, &p, &|_| Ok(serde_json::json!({"text": "y".repeat(50_000)})));
        assert_eq!(ran.len(), 1, "der zweite Aufruf läuft ohne Platz nicht");
        let first = c.messages().iter().find(|m| m.role == Role::Tool).unwrap();
        assert!(!first.content.contains("\"elided\""), "ungesehenes Ergebnis bleibt");
        assert!(c.paused && c.pause_note.is_some());
    }

    /// „Denkt nach“ gilt nur für die laufende Antwort; eine neue Anfrage beginnt ohne und misst ihre Wartezeit neu (#299).
    #[test]
    fn thinking_flag_and_wait_start_belong_to_the_turn() {
        let b = Script::new(vec![vec![TurnEvent::Thinking], vec![TurnEvent::Delta("x".into()), TurnEvent::Done]]);
        let mut c = ChatState::new(ChatFile::default(), None);
        assert!(!c.thinking && c.turn_started.is_none());
        c.draft = "q".into();
        c.send(&b, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {}));
        let started = c.turn_started.expect("Beginn gemerkt");
        c.poll();
        // Das Drehbuch-Backend lässt den Kanal fallen: die Antwort endet mit „brach unerwartet ab“ – sichtbar, nie still.
        assert!(c.thinking, "Denk-Ausgabe gemerkt");
        assert!(!c.streaming() && c.has_error(), "{:?}", c.messages());
        c.draft = "again".into();
        assert!(c.send(&b, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {})));
        assert!(!c.thinking, "neue Anfrage ohne „denkt nach“");
        assert!(c.turn_started.unwrap() >= started);
        c.poll();
        assert_eq!(c.messages().last().unwrap().content, "x");
    }

    #[test]
    fn token_scale_follows_the_servers_prompt_tokens() {
        let b = Script::new(vec![vec![TurnEvent::Budget { history: 1_000, prompt: 100 }, TurnEvent::Delta("a".into()), TurnEvent::Usage { prompt_tokens: Some(250), completion_tokens: None }, TurnEvent::Done]]);
        let mut c = ChatState::new(ChatFile::default(), None);
        c.draft = "q".into();
        c.send(&b, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {}));
        c.poll();
        assert!((c.token_scale - 2.5).abs() < 1e-6);
        // Der Platz wurde noch mit Faktor 1 gerechnet: jetzt gilt nur noch 1000 / 2,5
        let expect = ((1_000.0f64 / 2.5) as usize).saturating_sub(c.round_tokens()) * 3 / 2;
        assert_eq!(c.result_cap(), expect.min(tools::MAX_RESULT_BYTES));
        // Weniger als geschätzt: nie unter 1; mehr als 4: höchstens 4
        for (real, want) in [(50u32, 1.0f32), (10_000, 4.0)] {
            let b = Script::new(vec![vec![TurnEvent::Budget { history: 1_000, prompt: 100 }, TurnEvent::Usage { prompt_tokens: Some(real), completion_tokens: None }, TurnEvent::Done]]);
            let mut c = ChatState::new(ChatFile::default(), None);
            c.draft = "q".into();
            c.send(&b, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {}));
            c.poll();
            assert_eq!(c.token_scale, want);
        }
    }

    #[test]
    fn with_one_step_the_model_still_sees_the_denial() {
        let b = Script::new(vec![vec![tc("x", "no_such_tool", serde_json::json!({})), TurnEvent::Done]]);
        let p = policy(crate::mcp::McpLevel::Read, 1);
        let mut c = ChatState::new(ChatFile::default(), None);
        c.draft = "go".into();
        c.send(&b, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {}));
        drive(&mut c, &b, &p, &|_| panic!("unbekannt"));
        assert!(c.paused);
        assert_eq!(c.messages().iter().filter(|m| m.role == Role::Assistant).count(), 2, "zweite Anfrage mit der Ablehnung");
        assert_eq!(c.pause_note, None, "hier greift die Höchstzahl („Nach 1 Werkzeugaufrufen angehalten“)");
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

    #[test]
    fn context_notice_is_shown_once_per_chat() {
        struct Notice;
        impl ChatBackend for Notice {
            fn start(&self, _: &ChatMeta, _: &[ChatMsg], _: &TurnContext, _: Repaint) -> TurnHandle {
                let (h, tx, _) = TurnHandle::pair();
                for ev in [TurnEvent::ContextNotice("small".into()), TurnEvent::Delta("x".into()), TurnEvent::Done] {
                    tx.send(ev).unwrap();
                }
                h
            }
        }
        let mut c = ChatState::new(ChatFile::default(), None);
        for q in ["a", "b"] {
            c.draft = q.into();
            assert!(c.send(&Notice, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {})));
            assert!(c.poll().finished);
        }
        assert_eq!(c.messages()[1].note.as_deref(), Some("small"));
        assert!(c.messages()[3].note.is_none(), "nur einmal je Chat");
    }

    // ---- #340: als Text geschriebene Aufrufe, normalisierte Argumente, Schleifen-Schutz

    fn text(s: &str) -> Vec<TurnEvent> {
        vec![TurnEvent::Delta(s.into()), TurnEvent::Done]
    }

    fn start(c: &mut ChatState, b: &dyn ChatBackend, q: &str) {
        c.draft = q.into();
        assert!(c.send(b, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {})));
    }

    #[test]
    fn a_text_tool_call_runs_like_a_native_one() {
        // qwen schreibt den Aufruf als Text (Ollama hat ihn nicht erkannt)
        let b = Script::new(vec![text("Checking.\n<tool_call>\n{\"name\": \"read_terminal\", \"arguments\": {\"session\": \"3\", \"lines\": \"20\"}}\n</tool_call>"), text("Done.")]);
        let mut c = ChatState::new(ChatFile::default(), None);
        start(&mut c, &b, "read terminal 3");
        let seen = std::sync::Mutex::new(Vec::new());
        let ran = drive(&mut c, &b, &policy(crate::mcp::McpLevel::Read, 10), &|call| {
            seen.lock().unwrap().push(call.arguments.clone());
            Ok(serde_json::json!({"text": "hello"}))
        });
        assert_eq!(ran, ["read_terminal"]);
        assert_eq!(seen.lock().unwrap()[0], serde_json::json!({"session": 3, "lines": 20}), "normalisiert ausgeführt");
        let a = &c.messages()[1];
        assert_eq!(a.content, "Checking.", "der Aufruf-Text fällt aus der Antwort");
        assert_eq!(a.tool_calls.len(), 1);
        assert_eq!(a.tool_calls[0].arguments, serde_json::json!({"session": 3, "lines": 20}), "gespeichert wird die korrigierte Fassung");
        let r = &a.repairs[0];
        assert_eq!((r.id.as_str(), r.text_call), (a.tool_calls[0].id.as_str(), true));
        assert_eq!(r.fixes.len(), 2, "{:?}", r.fixes);
        assert_eq!(c.messages()[2].status, Some(ToolStatus::Ok));
        assert_eq!(c.messages().last().unwrap().content, "Done.");
        assert!(!c.busy());
        consistent(&c);
        // Gespeichert und wieder geladen: die Reparatur bleibt
        let back = super::super::store::parse(std::str::from_utf8(&super::super::store::serialize_capped(&c.file, 1 << 20)).unwrap()).unwrap().0;
        assert_eq!(back.messages[1].repairs, c.messages()[1].repairs);
    }

    #[test]
    fn text_calls_need_tools_and_prose_stays_untouched() {
        let call = "<tool_call>{\"name\": \"list_sessions\", \"arguments\": {}}</tool_call>";
        // Stufe Aus / Modell ohne Werkzeuge: der Text bleibt, nichts läuft
        for p in [policy(crate::mcp::McpLevel::Off, 10), {
            let mut p = policy(crate::mcp::McpLevel::Read, 10);
            p.tools_supported = Some(false);
            p
        }] {
            let b = Script::new(vec![text(call)]);
            let mut c = ChatState::new(ChatFile::default(), None);
            start(&mut c, &b, "q");
            assert!(drive(&mut c, &b, &p, &|_| panic!("ohne Werkzeuge")).is_empty());
            assert_eq!((c.messages().len(), c.messages()[1].content.as_str()), (2, call));
            assert!(!c.busy() && c.messages()[1].tool_calls.is_empty());
        }
        // Text über JSON ohne Aufruf: unverändert
        let prose = "The config looks like ```json\n{\"name\": \"x\", \"arguments\": {}}\n``` – that is all.";
        let b = Script::new(vec![text(prose)]);
        let mut c = ChatState::new(ChatFile::default(), None);
        start(&mut c, &b, "q");
        assert!(drive(&mut c, &b, &policy(crate::mcp::McpLevel::Read, 10), &|_| panic!("kein Aufruf")).is_empty());
        assert_eq!(c.messages()[1].content, prose);
        assert!(!c.busy() && c.status() == ChatStatus::Ready);
        // Gleich nach dem Ende einer Antwort mit `{` (vor dem nächsten Schritt) gilt der Chat noch als beschäftigt …
        let b = Script::new(vec![text("a set {x}")]);
        let mut c = ChatState::new(ChatFile::default(), None);
        start(&mut c, &b, "q");
        c.poll();
        assert!(c.busy(), "Textprüfung steht noch aus");
        assert_eq!(c.tool_step(&policy(crate::mcp::McpLevel::Read, 10)), ToolStep::Idle);
        assert!(!c.busy());
        // … gewöhnlicher Text ohne JSON braucht keinen weiteren Schritt
        let b = Script::new(vec![text("hello")]);
        let mut c = ChatState::new(ChatFile::default(), None);
        start(&mut c, &b, "q");
        c.poll();
        assert!(!c.busy());
    }

    #[test]
    fn text_calls_get_the_same_approval_and_safety_checks() {
        // Schreibendes Werkzeug als Text (Mistral): wartet auf Freigabe wie ein echter Aufruf
        let b = Script::new(vec![text("[TOOL_CALLS]send_text[ARGS]{\"session\": \"2\", \"text\": \"dir\"}"), text("ok")]);
        let p = policy(crate::mcp::McpLevel::Control, 10);
        let mut c = ChatState::new(ChatFile::default(), None);
        start(&mut c, &b, "type dir");
        assert!(drive(&mut c, &b, &p, &|_| panic!("erst nach Freigabe")).is_empty());
        assert!(c.awaiting_approval());
        assert_eq!(c.pending_approval().unwrap().arguments, serde_json::json!({"session": 2, "text": "dir"}));
        // Stufe Lesen: verboten
        let b = Script::new(vec![text("[TOOL_CALLS]send_text[ARGS]{\"session\": 2, \"text\": \"dir\"}"), text("ok")]);
        let mut c = ChatState::new(ChatFile::default(), None);
        start(&mut c, &b, "type dir");
        assert!(drive(&mut c, &b, &policy(crate::mcp::McpLevel::Read, 10), &|_| panic!("verboten")).is_empty());
        assert_eq!(c.messages()[2].status, Some(ToolStatus::Denied));
        // Von einem Agenten (Sitzung 7) ausgelöst: auch "7" als Text zielt nach der Normalisierung auf 7 → Umweg-Schutz
        let b = Script::new(vec![text("<tool_call>{\"name\": \"send_text\", \"arguments\": {\"session\": \"7\", \"text\": \"x\"}}</tool_call>"), text("ok")]);
        let mut c = ChatState::new(ChatFile::default(), None);
        start(&mut c, &b, "q");
        assert!(drive(&mut c, &b, &p.clone().with_origins(vec![7]), &|_| panic!("Umweg")).is_empty());
        assert!(!c.awaiting_approval());
        assert!(c.messages()[2].content.contains(tools::PROXY_MESSAGE), "{}", c.messages()[2].content);
        // … und keine Claude-Sitzung, auch nicht mit "true" als Text
        let b = Script::new(vec![text("<tool_call>{\"name\": \"create_session\", \"arguments\": {\"claude\": \"true\"}}</tool_call>"), text("ok")]);
        let mut c = ChatState::new(ChatFile::default(), None);
        start(&mut c, &b, "q");
        assert!(drive(&mut c, &b, &p.clone().with_origins(vec![7]), &|_| panic!("Claude-Start")).is_empty());
        assert!(c.messages()[2].content.contains(tools::SPAWN_CLAUDE_MESSAGE));
    }

    #[test]
    fn broken_text_calls_and_unknown_tools_get_a_hint() {
        let b = Script::new(vec![
            text("<tool_call>{\"name\": \"read_terminal\", \"arguments\": {\"session\": 3,}}</tool_call>"),
            text("<tool_call>{\"name\": \"get_weather\", \"arguments\": {}}</tool_call>"),
            text("Sorry."),
        ]);
        let mut c = ChatState::new(ChatFile::default(), None);
        start(&mut c, &b, "q");
        assert!(drive(&mut c, &b, &policy(crate::mcp::McpLevel::Read, 10), &|_| panic!("darf nicht laufen")).is_empty());
        let res: Vec<&ChatMsg> = c.messages().iter().filter(|m| m.role == Role::Tool).collect();
        assert_eq!(res.len(), 2);
        assert!(res[0].content.contains("bad_args") && res[0].content.contains("not valid JSON") && res[0].content.contains("Correct example"), "{}", res[0].content);
        assert!(res[1].content.contains("unknown_tool") && res[1].content.contains("list_sessions"), "{}", res[1].content);
        assert_eq!(c.messages().last().unwrap().content, "Sorry.", "das Modell bekommt die Hinweise und antwortet");
        assert!(c.messages()[1].repairs[0].text_call);
        consistent(&c);
    }

    #[test]
    fn native_calls_are_normalised_and_the_fixes_recorded() {
        let b = Script::new(vec![vec![tc("a", "read_terminal", serde_json::json!({"session": "4", "screen": "true"})), tc("b", "whoami", serde_json::json!({})), TurnEvent::Done], text("ok")]);
        let mut c = ChatState::new(ChatFile::default(), None);
        start(&mut c, &b, "q");
        let seen = std::sync::Mutex::new(Vec::new());
        drive(&mut c, &b, &policy(crate::mcp::McpLevel::Read, 10), &|call| {
            seen.lock().unwrap().push(call.arguments.clone());
            ok_exec(call)
        });
        assert_eq!(seen.lock().unwrap()[0], serde_json::json!({"session": 4, "screen": true}));
        let a = &c.messages()[1];
        assert_eq!(a.repairs.len(), 1, "nur der korrigierte Aufruf");
        assert_eq!((a.repairs[0].id.as_str(), a.repairs[0].text_call), ("a", false));
        assert_eq!(a.repairs[0].fixes, [r#"screen: "true" → true"#, r#"session: "4" → 4"#]);
    }

    #[test]
    fn the_same_failing_call_three_times_in_a_row_stops_the_loop() {
        let bad = || vec![tc("x", "read_terminal", serde_json::json!({"session": "abc"})), TurnEvent::Done];
        let b = Script::new(vec![bad()]);
        let p = policy(crate::mcp::McpLevel::Read, 10);
        let mut c = ChatState::new(ChatFile::default(), None);
        start(&mut c, &b, "q");
        drive(&mut c, &b, &p, &|_| panic!("ungültig"));
        assert!(c.paused && c.loop_stop && !c.busy());
        assert_eq!(c.steps, LOOP_LIMIT);
        assert_eq!(c.pause_note.as_deref(), Some("The model sent the same failing tool call (read_terminal) 3 times in a row – stopped."));
        assert_eq!(c.messages().iter().filter(|m| m.role == Role::Tool).count(), 3);
        consistent(&c);
        // „Weiter“: kommt derselbe Fehler noch einmal, hält die Runde gleich wieder an
        assert!(c.resume() && !c.loop_stop);
        drive(&mut c, &b, &p, &|_| panic!("ungültig"));
        assert!(c.paused && c.loop_stop && c.steps == LOOP_LIMIT + 1);
        // Neue Nachricht: alles neu
        start(&mut c, &Script::new(vec![text("ok")]), "enough");
        assert!(!c.loop_stop && !c.paused);
        // Fehler bei der Ausführung zählen genauso
        let b = Script::new(vec![vec![tc("x", "list_sessions", serde_json::json!({})), TurnEvent::Done]]);
        let mut c = ChatState::new(ChatFile::default(), None);
        start(&mut c, &b, "q");
        assert_eq!(drive(&mut c, &b, &p, &|_| Err(("boom".into(), "fails".into()))).len(), 3);
        assert!(c.paused && c.loop_stop);
        // Andere Argumente oder ein Erfolg dazwischen: kein Anhalten durch den Schleifen-Schutz
        let turn = |s: &str| vec![tc("x", "read_terminal", serde_json::json!({"session": s})), TurnEvent::Done];
        let b = Script::new(vec![turn("a"), turn("b"), turn("a"), turn("b"), text("ok")]);
        let mut c = ChatState::new(ChatFile::default(), None);
        start(&mut c, &b, "q");
        drive(&mut c, &b, &p, &|_| panic!("ungültig"));
        assert!(!c.loop_stop && !c.paused, "{:?}", c.pause_note);
        let ok_then_fail = Script::new(vec![vec![tc("x", "list_sessions", serde_json::json!({})), TurnEvent::Done]]);
        let n = std::sync::atomic::AtomicU32::new(0);
        let mut c = ChatState::new(ChatFile::default(), None);
        start(&mut c, &ok_then_fail, "q");
        let ran = drive(&mut c, &ok_then_fail, &p, &|_| if n.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 2 == 0 { ok_exec(&ToolCall::default()) } else { Err(("boom".into(), "x".into())) });
        assert_eq!(ran.len(), 10, "bis zur Höchstzahl, nicht vom Schleifen-Schutz angehalten");
        assert!(c.paused && !c.loop_stop);
    }

    fn bg_event(id: &str) -> super::super::store::ChatEvent {
        super::super::store::ChatEvent { kind: "background_command".into(), id: id.into(), command: "cargo build".into(), state: "exited".into(), exit_code: Some(0), duration_ms: 1000, error: None }
    }

    /// Bis zum nächsten Schritt: Antwort abholen, Ergebnisse abholen, `tool_step`.
    fn step(c: &mut ChatState, p: &ToolPolicy) -> ToolStep {
        for _ in 0..100 {
            c.poll();
            c.poll_tools();
            if !c.streaming() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        c.tool_step(p)
    }

    /// `ask_user` (#356): der Aufruf läuft, bis der Nutzer absendet; solange „braucht dich“. Abbrechen sagt es dem Modell,
    /// Stopp räumt das Formular weg.
    #[test]
    fn ask_user_waits_for_the_answer_and_counts_as_needs_you() {
        let ask = serde_json::json!({"questions": [{"question": "Which?", "options": ["a", {"label": "b", "recommended": true}]}]});
        let b = Script::new(vec![vec![tc("q", "ask_user", ask.clone()), TurnEvent::Done], text("Thanks.")]);
        let p = policy(crate::mcp::McpLevel::Read, 10);
        let mut c = ChatState::new(ChatFile::default(), None);
        start(&mut c, &b, "help me choose");
        let ToolStep::Execute(call) = step(&mut c, &p) else { panic!("ask_user läuft ohne Freigabe") };
        // Wie der Hub: Formular mit dem Kanal des Aufrufs öffnen.
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let (qs, fixes) = super::super::ask::parse(&call.arguments).unwrap();
        c.open_ask(super::super::ask::AskForm::new(call.id.clone(), qs, fixes, Some(tx)));
        c.tool_started(rx);
        assert!(c.awaiting_answer());
        assert_eq!(c.status(), ChatStatus::Approval, "wartet auf den Nutzer wie eine Freigabe");
        assert!(!c.answer_ask(super::super::ask::AskReply::Submit), "nichts gewählt");
        c.ask.as_mut().unwrap().toggle(0, 0);
        assert_eq!(c.ask.as_ref().unwrap().questions[0].options[0].label, "b", "empfohlen zuerst");
        assert!(c.answer_ask(super::super::ask::AskReply::Submit));
        assert!(c.ask.is_none() && !c.awaiting_answer());
        assert_eq!(step(&mut c, &p), ToolStep::Request);
        let res = c.messages().iter().find(|m| m.role == Role::Tool).unwrap();
        assert_eq!(res.status, Some(ToolStatus::Ok));
        assert!(res.content.contains("\"selected\":[\"b\"]"), "{}", res.content);
        // Abbrechen
        let b = Script::new(vec![vec![tc("q", "ask_user", ask.clone()), TurnEvent::Done], text("ok")]);
        let mut c = ChatState::new(ChatFile::default(), None);
        start(&mut c, &b, "again");
        let ToolStep::Execute(call) = step(&mut c, &p) else { panic!() };
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let (qs, _) = super::super::ask::parse(&call.arguments).unwrap();
        c.open_ask(super::super::ask::AskForm::new(call.id.clone(), qs, vec![], Some(tx)));
        c.tool_started(rx);
        assert!(c.answer_ask(super::super::ask::AskReply::Cancel));
        step(&mut c, &p);
        let res = c.messages().iter().find(|m| m.role == Role::Tool).unwrap();
        assert!(res.content.contains("\"cancelled\":true"), "{}", res.content);
        // Stopp während die Fragen offen sind
        let b = Script::new(vec![vec![tc("q", "ask_user", ask), TurnEvent::Done]]);
        let mut c = ChatState::new(ChatFile::default(), None);
        start(&mut c, &b, "third");
        let ToolStep::Execute(call) = step(&mut c, &p) else { panic!() };
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let (qs, _) = super::super::ask::parse(&call.arguments).unwrap();
        c.open_ask(super::super::ask::AskForm::new(call.id.clone(), qs, vec![], Some(tx)));
        c.tool_started(rx);
        c.stop();
        assert!(c.ask.is_none() && c.status() == ChatStatus::Ready);
        consistent(&c);
    }

    /// Nachrichten von Paneldeck (#355): sofort im Verlauf, wenn nichts läuft (keine neue Anfrage); während eines
    /// Werkzeugaufrufs warten sie bis nach dem Ergebnis; kommen sie während der Runde, fragt der Chat danach noch einmal.
    #[test]
    fn events_wait_for_a_consistent_history_and_resume_a_running_round() {
        let p = policy(crate::mcp::McpLevel::Read, 10);
        // Ruhend: sofort, keine Anfrage
        let b = Script::new(vec![text("Started it.")]);
        let mut c = ChatState::new(ChatFile::default(), None);
        start(&mut c, &b, "build");
        assert_eq!(step(&mut c, &p), ToolStep::Idle);
        c.push_event(bg_event("bg1"), "[Paneldeck] Background command bg1 finished".into());
        let last = c.messages().last().unwrap();
        assert_eq!((last.role, last.event.as_ref().map(|e| e.id.as_str())), (Role::System, Some("bg1")));
        assert!(!c.events_pending());
        assert_eq!(step(&mut c, &p), ToolStep::Idle, "Runde war vorbei: das Modell sieht es mit der nächsten Nachricht");
        // Die Nachricht geht als Nachricht des Nutzers an das Modell.
        let (sent, _) = super::super::llm_backend::map_history(c.messages());
        assert_eq!(sent.last().map(|m| (m.role, m.content.as_str())), Some((crate::llm::Role::User, "[Paneldeck] Background command bg1 finished")));
        // Während ein Werkzeug läuft: erst nach dem Ergebnis, vor der nächsten Anfrage
        let b = Script::new(vec![vec![tc("w", "list_sessions", serde_json::json!({})), TurnEvent::Done], text("Done.")]);
        let mut c = ChatState::new(ChatFile::default(), None);
        start(&mut c, &b, "go");
        let ToolStep::Execute(_) = step(&mut c, &p) else { panic!() };
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        c.tool_started(rx);
        c.push_event(bg_event("bg2"), "bg2 finished".into());
        assert!(c.events_pending(), "Aufruf offen: wartet");
        tx.send(ok_exec(&ToolCall::default())).unwrap();
        assert_eq!(step(&mut c, &p), ToolStep::Request);
        let roles: Vec<Role> = c.messages().iter().map(|m| m.role).collect();
        assert_eq!(roles, [Role::User, Role::Assistant, Role::Tool, Role::System], "Ergebnis direkt nach dem Aufruf, dann die Nachricht");
        consistent(&c);
        // Während das Modell schreibt (und dann ohne Aufrufe endet): danach noch eine Anfrage
        let b = Script::new(vec![text("I will wait for the build."), text("The build passed.")]);
        let mut c = ChatState::new(ChatFile::default(), None);
        start(&mut c, &b, "build it");
        assert!(c.streaming());
        c.push_event(bg_event("bg3"), "bg3 finished".into());
        assert!(c.events_pending());
        assert_eq!(step(&mut c, &p), ToolStep::Request, "die Runde lief: das Modell macht weiter");
        assert_eq!(c.messages().last().unwrap().event.as_ref().unwrap().id, "bg3");
        c.next_request(&b, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {}));
        assert_eq!(step(&mut c, &p), ToolStep::Idle);
        assert_eq!(c.messages().last().unwrap().content, "The build passed.");
        // Nach einer fehlgeschlagenen Antwort nicht von selbst weiter
        struct Fail;
        impl ChatBackend for Fail {
            fn start(&self, _: &ChatMeta, _: &[ChatMsg], _: &TurnContext, _: Repaint) -> TurnHandle {
                let (h, tx, _) = TurnHandle::pair();
                tx.send(TurnEvent::Error("refused".into())).unwrap();
                h
            }
        }
        let mut c = ChatState::new(ChatFile::default(), None);
        start(&mut c, &Fail, "x");
        c.push_event(bg_event("bg4"), "bg4 finished".into());
        assert_eq!(step(&mut c, &p), ToolStep::Idle);
        assert!(!c.events_pending());
        // Neue Nachricht des Nutzers: wartende Nachrichten kommen davor
        let b = Script::new(vec![vec![tc("w", "list_sessions", serde_json::json!({})), TurnEvent::Done], text("ok")]);
        let mut c = ChatState::new(ChatFile::default(), None);
        start(&mut c, &b, "go");
        let ToolStep::Execute(_) = step(&mut c, &p) else { panic!() };
        let (_tx, rx) = std::sync::mpsc::sync_channel(1);
        c.tool_started(rx);
        c.push_event(bg_event("bg5"), "bg5 finished".into());
        c.stop();
        assert!(!c.events_pending());
        assert_eq!(c.messages().last().unwrap().role, Role::System, "nach Stopp im Verlauf");
        consistent(&c);
        // Angehalten (Höchstzahl) mit wartenden Aufrufen: die Nachricht wartet im Speicher, steht aber in der Datei.
        let b = Script::new(vec![vec![tc("a", "list_sessions", serde_json::json!({})), tc("b", "list_sessions", serde_json::json!({})), TurnEvent::Done], text("ok")]);
        let p1 = policy(crate::mcp::McpLevel::Read, 1);
        let mut c = ChatState::new(ChatFile::default(), None);
        start(&mut c, &b, "go");
        let ToolStep::Execute(_) = step(&mut c, &p1) else { panic!() };
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        tx.send(ok_exec(&ToolCall::default())).unwrap();
        c.tool_started(rx);
        assert_eq!(step(&mut c, &p1), ToolStep::Idle);
        assert!(c.paused);
        c.push_event(bg_event("bg6"), "bg6 finished".into());
        assert!(c.events_pending());
        let saved = c.file_for_save();
        assert!(saved.messages.last().unwrap().event.as_ref().is_some_and(|e| e.id == "bg6"), "in der Datei");
        let mut m = saved.messages.clone();
        assert_eq!(logic::close_orphans(&mut m, tools::interrupted), 0, "Datei stimmig");
    }
}
