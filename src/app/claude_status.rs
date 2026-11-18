//! Status einer Claude-Sitzung über die Hook-Meldungen hinaus.
//!
//! Claude Code meldet nicht jedes Ende: kein `Stop` nach einer Unterbrechung (Esc), nach einem API-Fehler oder einer im
//! Terminal abgelehnten Freigabe; keine Meldung, wenn der Prozess abgeschossen wird; nach einer im Terminal erteilten
//! Freigabe erst wieder, wenn das Werkzeug fertig ist. Ohne Gegenprobe bliebe „arbeitet“ bzw. „Freigabe?“ stehen, bis
//! der nächste Prompt durch ist. Hier laufen deshalb zusammen:
//!
//! * der Rückfall je Sitzung ([`App::claude_status_tick`] → [`crate::hooks::stale_status`]) aus Transkript
//!   (`usage::Tail`), Ruhe des Terminals und Bildschirm;
//! * das Ende von Claude am Prompt der Shell ([`Entry::shell_prompt_seen`], Shell-Integration): Status weg, und ein im
//!   selben Terminal neu gestarteter Claude darf sich wieder zuordnen;
//! * die Annahme einer Hook-Meldung für eine Sitzung ([`Entry::take_hook`]) samt der Regel, dass eine offene Rückfrage
//!   nicht von der Meldung eines anderen, gleichzeitig laufenden Werkzeugs überschrieben wird.

use super::*;
use crate::hooks::{ConvMatch, Observed, ScreenHints, TurnState};

/// So viel früher als die letzte Meldung darf der Beleg im Transkript datiert sein: Die Meldung kommt um die Laufzeit des
/// Hook-Aufrufs verspätet im Hub an, das Transkript trägt die Uhrzeit von Claude.
const EVIDENCE_SLACK_MS: u64 = 2000;
/// So lange nach dem Prompt der Shell zählen Meldungen des beendeten Claude noch als Nachzügler (verworfen).
const STRAGGLER: Duration = Duration::from_secs(3);
/// Abstand der Gegenprobe (Bildschirm lesen, Transkript-Stand holen).
const CHECK_EVERY: Duration = Duration::from_millis(500);
/// Ein zweiter Stop so kurz nach dem ersten, ohne Meldung dazwischen, ist derselbe noch einmal.
const DUPLICATE_STOP: Duration = Duration::from_secs(1);

/// Was der Hub je Start über den Claude einer Sitzung mitführt.
#[derive(Default)]
pub(super) struct Track {
    pub(super) turn: TurnState,
    /// Letzte angenommene Hook-Meldung bzw. vom Hub selbst gesetzter Status: Zeitpunkt und Uhrzeit (ms seit 1970, zum
    /// Vergleich mit den Zeitstempeln im Transkript).
    pub(super) evt: Option<(Instant, u64)>,
    /// `Session::input_stamp` bei der letzten Meldung: Eingaben danach können Claude unterbrochen oder eine Rückfrage
    /// beantwortet haben.
    pub(super) input_base: u64,
    /// Werkzeug, für das die offene Freigabe-Frage gestellt wurde (aus der Benachrichtigung), sonst leer.
    pub(super) perm_tool: String,
    /// Zeilen des Startbefehls (Startbefehl des Profils, `claude …`): So viele Prompts zeigt die Shell, bevor der
    /// letzte davon läuft; jeder weitere heißt, dass er beendet ist.
    pub(super) launch_lines: u64,
    /// Zuletzt gesehener Prompt-Zähler der Shell.
    pub(super) shell_seq: u64,
    /// Die Shell stand nach dem Start wieder an ihrem Prompt (Claude beendet): wann.
    pub(super) gone: Option<(Instant, std::time::SystemTime)>,
    /// Die letzte angenommene Meldung war ein Stop: wann.
    pub(super) last_stop: Option<Instant>,
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Prompt-Zähler der Shell: OSC 133;A (PowerShell, bash), sonst die Ordnermeldung, die cmd mit jedem Prompt schickt.
fn shell_seq(st: &crate::terminal::ShellState) -> u64 {
    if st.prompt_seq > 0 { st.prompt_seq } else { st.cwd_seq }
}

/// Zeilen eines Startbefehls (jede wird mit Enter abgeschickt).
pub(super) fn launch_lines(initial: Option<&str>) -> u64 {
    initial.map_or(0, |i| i.bytes().filter(|b| *b == b'\r').count() as u64)
}

/// Belegt das Transkript, dass die Runde nach der letzten Meldung zu Ende ging? `tail` = letzter Eintrag und wann er
/// gelesen wurde, `evt` = letzte Meldung. Der Eintrag muss nach der Meldung gelesen sein (sonst ist er vielleicht schon
/// überholt) und darf nicht aus einer früheren Runde stammen.
pub(super) fn turn_ended(tail: Option<(crate::usage::Tail, Instant)>, evt: Option<(Instant, u64)>) -> bool {
    let (Some((tail, checked)), Some((at, wall))) = (tail, evt) else { return false };
    tail.kind.ended() && checked > at && tail.at_ms.is_some_and(|ms| ms + EVIDENCE_SLACK_MS >= wall)
}

/// Ergebnis einer angenommenen Hook-Meldung.
pub(super) struct HookEffect {
    pub(super) old: ClaudeStatus,
    pub(super) new: ClaudeStatus,
    pub(super) conv: ConvMatch,
    /// Meldung ohne Schlüssel (globale Hooks einer älteren paneldeck.exe): nur für den Status.
    pub(super) keyless: bool,
}

impl Entry {
    /// Eine Meldung oder ein vom Hub gesetzter Status: ab jetzt zählt die Zeit für den Rückfall.
    pub(super) fn stamp_event(&mut self) {
        self.track.evt = Some((Instant::now(), unix_ms()));
        self.track.input_base = self.live.as_ref().map_or(0, |s| s.input_stamp());
    }

    /// Der Hub hat Claude gerade etwas zu tun gegeben (Prompt aus der Warteschlange, `send_text`, Freigabe erteilt):
    /// „arbeitet“ gilt sofort, der Hook kommt erst Sekundenbruchteile später. War Claude bereit und bestätigt keine
    /// Meldung die Annahme (der Text ging in einen Dialog, war ein lokaler Befehl wie `/model`), gilt wieder „bereit“.
    pub(super) fn assume_working(&mut self) {
        self.track.turn.tentative = (self.status == ClaudeStatus::Idle).then_some(ClaudeStatus::Idle);
        // Die Runde gilt als begonnen: Ein Hintergrund-Agent, der gerade jetzt fertig wird, macht daraus kein „bereit“.
        self.track.turn.open = true;
        self.status = ClaudeStatus::Working;
        self.stamp_event();
    }

    /// Hook-Meldung für diese Sitzung annehmen: Schlüssel und Konversation prüfen (#260), Status fortschreiben. `None` =
    /// verworfen. Transkript, Ordner, Warteschlange und Benachrichtigungen macht der Aufrufer (`App::poll_hooks`).
    pub(super) fn take_hook(&mut self, m: &HookMsg) -> Option<HookEffect> {
        // Nachzügler des beendeten Claude (z. B. ein Stop, das erst nach dem Prompt der Shell ankommt) zählen nicht;
        // ein neuer Claude meldet sich zuerst mit SessionStart.
        if m.event != "SessionStart" && self.track.gone.is_some_and(|(t, _)| t.elapsed() < STRAGGLER) {
            return None;
        }
        // Nur Meldungen des laufenden Starts (#260): Schlüssel `PANELDECK_SESSION_KEY` dieses Starts. Ein anderes
        // Terminal, ein übrig gebliebener Prozess eines früheren Starts oder eine Fälschung mit der bloßen Nummer
        // ändert weder Status noch Konversation.
        // Ausnahme: Meldungen ganz ohne Schlüssel stammen von globalen Hooks einer älteren paneldeck.exe (Hinweis in den
        // Einstellungen). Sie zählen nur für den Status, solange in diesem Start keine Meldung mit Schlüssel kam, und
        // ordnen nie eine Konversation zu.
        let keyless = match hooks::admit(&self.session_key, self.hook_keyed, &self.hook_conv, m) {
            hooks::Admit::Drop => return None,
            hooks::Admit::Keyed => {
                self.hook_keyed = true;
                false
            }
            hooks::Admit::StatusOnly => true,
        };
        // Eine andere Konversation als die dieses Starts (z. B. `claude -p` aus der Sitzung heraus) zählt nicht.
        let conv = if keyless { ConvMatch::NoId } else { self.hook_conv.feed(m) };
        if conv == ConvMatch::Foreign {
            return None;
        }
        // Derselbe Stop doppelt zugestellt (die Hooks stehen in zwei Settings-Dateien und Claude legt sie nicht
        // zusammen): Der zweite träfe auf den Prompt, den die Warteschlange nach dem ersten geschickt hat, und schickte
        // gleich den übernächsten los.
        if m.event == "Stop" {
            if self.track.last_stop.is_some_and(|t| t.elapsed() < DUPLICATE_STOP) {
                return None;
            }
            self.track.last_stop = Some(Instant::now());
        } else {
            self.track.last_stop = None;
        }
        let old = self.status;
        self.hook_seen = true;
        if m.event == "SessionStart" {
            self.track.gone = None;
        }
        let mut new = hooks::status_after(old, &mut self.track.turn, m);
        // Offene Rückfrage: Die Meldung eines anderen, gleichzeitig laufenden Werkzeugs oder Unter-Agenten beendet sie
        // nicht (`hooks::tool_event_answers`) – Claude meldet die Frage kein zweites Mal.
        let asking = |s: ClaudeStatus| matches!(s, ClaudeStatus::Permission | ClaudeStatus::Input);
        if asking(old) && new == ClaudeStatus::Working && matches!(m.event.as_str(), "PreToolUse" | "PostToolUse") {
            let (input_since, screen) = match &self.live {
                Some(s) => (s.input_stamp() > self.track.input_base, hooks::screen_hints(&s.screen_text())),
                None => (true, ScreenHints::default()),
            };
            if !hooks::tool_event_answers(m, &self.track.perm_tool, input_since, screen) {
                new = old;
            }
        }
        self.status = new;
        self.stamp_event();
        // Eine (neue) Rückfrage: ab hier zählen Eingaben als mögliche Antwort; kurze Sperre gegen Fehlklicks.
        if asking(new) && (new != old || hooks::notification_asks(m)) {
            self.perm_since = Some(Instant::now());
            self.track.perm_tool = m.message.as_deref().and_then(hooks::permission_tool).unwrap_or("").to_string();
            self.status_msg = m.message.clone().unwrap_or_default();
        } else if new != old {
            self.status_msg = m.message.clone().unwrap_or_default();
        }
        Some(HookEffect { old, new, conv, keyless })
    }

    /// Die Shell zeigt nach dem Startbefehl wieder ihren Prompt: Claude ist beendet (auch ohne SessionEnd –
    /// abgeschossen, abgestürzt, Meldung verloren). Der Status fällt weg; die Warteschlange tippt so nie in die Shell.
    pub(super) fn shell_prompt_seen(&mut self) {
        self.hook_conv.shell_prompt();
        self.track.gone = Some((Instant::now(), std::time::SystemTime::now()));
        self.track.turn = TurnState::default();
        if self.status != ClaudeStatus::Unknown {
            self.status = ClaudeStatus::Unknown;
            self.status_msg.clear();
            self.perm_since = None;
        }
    }

    /// Neuen Prompt-Zähler der Shell auswerten; `true`, wenn damit das Ende von Claude erkannt wurde.
    pub(super) fn note_shell_seq(&mut self, seq: u64) -> bool {
        if seq <= self.track.shell_seq {
            return false;
        }
        self.track.shell_seq = seq;
        if seq <= self.track.launch_lines {
            return false; // Prompts vor dem Startbefehl
        }
        self.shell_prompt_seen();
        true
    }

    /// Zählt das Transkript für den Status ohne Hooks? Nach dem Ende von Claude erst wieder, wenn danach
    /// hineingeschrieben wurde (ein neuer Claude läuft).
    pub(super) fn transcript_status_ok(&mut self, modified: Option<std::time::SystemTime>) -> bool {
        match self.track.gone {
            Some((_, wall)) if modified.is_none_or(|m| m <= wall) => false,
            Some(_) => {
                self.track.gone = None;
                true
            }
            None => true,
        }
    }

    /// Beobachtungen für den Rückfall. Den Bildschirm liest das nur, wenn es darauf ankommen kann.
    fn observe(&self, tail: Option<(crate::usage::Tail, Instant)>) -> Option<Observed> {
        let s = self.live.as_ref()?;
        let since_event = self.track.evt.map_or(Duration::MAX, |(t, _)| t.elapsed());
        let quiet = s.quiet_for();
        let stamp = s.input_stamp();
        let input_since = (stamp > self.track.input_base).then(|| Duration::from_millis(crate::terminal::mono_ms().saturating_sub(stamp)));
        let turn_ended = turn_ended(tail, self.track.evt);
        let need_screen = match self.status {
            ClaudeStatus::Working => since_event >= hooks::STALE_SETTLE && quiet >= hooks::STALE_SETTLE,
            ClaudeStatus::Permission | ClaudeStatus::Input => input_since.is_some(),
            ClaudeStatus::Unknown => turn_ended,
            ClaudeStatus::Idle => false,
        };
        let screen = if need_screen { hooks::screen_hints(&s.screen_text()) } else { ScreenHints::default() };
        Some(Observed { since_event, quiet, screen, input_since, turn_ended })
    }

    /// Status aus dem Rückfall übernehmen. Anders als ein `Stop` schickt das nie den nächsten Prompt der Warteschlange
    /// los und meldet nichts: Nach einer Unterbrechung oder einem Fehler entscheidet der Nutzer, wie es weitergeht.
    pub(super) fn apply_fallback(&mut self, new: ClaudeStatus) {
        self.status = new;
        self.status_msg.clear();
        self.perm_since = None;
        self.track.turn.tentative = None;
        // Unbekannt, weil Meldungen ausblieben: ein späterer Beleg im Transkript macht daraus noch „bereit“.
        self.track.turn.stale = new == ClaudeStatus::Unknown;
        if new == ClaudeStatus::Working {
            // Ab jetzt zählt die Zeit neu. Sonst bleibt die letzte Meldung der Bezug für Belege aus dem Transkript.
            self.stamp_event();
        } else {
            self.track.turn.open = false;
        }
    }

    /// Gegenprobe für eine Sitzung; `true`, wenn sich der Status geändert hat.
    fn stale_tick(&mut self, tail: Option<(crate::usage::Tail, Instant)>) -> bool {
        let Some(o) = self.observe(tail) else { return false };
        match hooks::stale_status(self.status, &self.track.turn, &o) {
            Some(new) => {
                self.apply_fallback(new);
                true
            }
            None => false,
        }
    }
}

impl App {
    /// Je Bild (Hauptdurchgang): Ende von Claude am Prompt der Shell erkennen und, in größeren Abständen, den Status
    /// gegenprüfen, wenn Hook-Meldungen ausbleiben.
    pub(super) fn claude_status_tick(&mut self, ctx: &egui::Context) {
        let check = self.last_status_check.elapsed() >= CHECK_EVERY;
        if check {
            self.last_status_check = Instant::now();
        }
        let mut changed = false;
        for (id, e) in self.entries.iter_mut() {
            let Some(s) = e.live.as_ref() else { continue };
            let seq = shell_seq(&s.shell_state.lock().unwrap_or_else(|x| x.into_inner()));
            let exited = s.has_exited();
            changed |= e.note_shell_seq(seq);
            // Nur mit Hook-Meldungen: ohne sie kommt der Status aus dem Transkript (`sync_watchers`).
            let stale = match e.status {
                ClaudeStatus::Working | ClaudeStatus::Permission | ClaudeStatus::Input => true,
                ClaudeStatus::Unknown => e.track.turn.stale,
                ClaudeStatus::Idle => false,
            };
            if check && stale && e.hook_seen && !exited {
                changed |= e.stale_tick(self.usage.tail(*id));
            }
        }
        if changed {
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{Tail, TailKind};

    fn claude_entry() -> Entry {
        let mut e = Entry::new(SessionMeta { id: 1, claude: true, ..SessionMeta::default() });
        e.session_key = "k".into();
        e.hook_conv = hooks::ConvBinding::launch(Some("A".into()), false);
        e.track.launch_lines = 1;
        e
    }

    fn msg(event: &str) -> HookMsg {
        HookMsg { session: 1, event: event.into(), claude_session: Some("A".into()), key: Some("k".into()), ..HookMsg::default() }
    }

    fn feed(e: &mut Entry, event: &str) -> ClaudeStatus {
        e.take_hook(&msg(event)).map(|f| f.new).unwrap_or(e.status)
    }

    /// Der gemeldete Fehler: Nach Esc kommt kein Stop – ohne Rückfall bliebe „arbeitet“ stehen.
    #[test]
    fn an_interrupted_turn_leaves_working_with_evidence_from_the_transcript() {
        let mut e = claude_entry();
        feed(&mut e, "SessionStart");
        assert_eq!(feed(&mut e, "UserPromptSubmit"), ClaudeStatus::Working);
        assert_eq!(feed(&mut e, "PreToolUse"), ClaudeStatus::Working);
        let (at, wall) = e.track.evt.unwrap();
        let calm = Observed { since_event: Duration::from_secs(5), quiet: Duration::from_secs(5), ..Observed::default() };
        // Ohne Beleg und ohne lange Ruhe bleibt es bei „arbeitet“ (ein langes Werkzeug meldet zwischendurch nichts)
        assert_eq!(hooks::stale_status(e.status, &e.track.turn, &calm), None);
        // Beleg: die Unterbrechung steht nach der letzten Meldung im Transkript und wurde danach gelesen
        let interrupted = Tail { kind: TailKind::Interrupted, at_ms: Some(wall + 800) };
        let later = at + Duration::from_millis(900);
        assert!(turn_ended(Some((interrupted, later)), e.track.evt));
        let o = Observed { turn_ended: true, ..calm };
        assert_eq!(hooks::stale_status(e.status, &e.track.turn, &o), Some(ClaudeStatus::Idle));
        e.apply_fallback(ClaudeStatus::Idle);
        assert_eq!(e.status, ClaudeStatus::Idle);
        assert!(!e.track.turn.open, "die Runde ist zu");
        assert_eq!(e.track.evt, Some((at, wall)), "die letzte Meldung bleibt der Bezug für Belege");
        // Ohne Beleg erst unbekannt; kommt er später doch, wird daraus bereit
        feed(&mut e, "UserPromptSubmit");
        e.apply_fallback(ClaudeStatus::Unknown);
        assert!(e.track.turn.stale);
        let o = Observed { turn_ended: true, ..calm };
        assert_eq!(hooks::stale_status(e.status, &e.track.turn, &o), Some(ClaudeStatus::Idle));
        // Am Prompt der Shell (Claude wirklich beendet) gilt das nicht mehr
        e.shell_prompt_seen();
        assert_eq!(hooks::stale_status(e.status, &e.track.turn, &o), None);
        // Ein Beleg aus einer früheren Runde oder einer, der vor der Meldung gelesen wurde, zählt nicht
        assert!(!turn_ended(Some((Tail { kind: TailKind::Interrupted, at_ms: Some(wall - 60_000) }, later)), Some((at, wall))));
        assert!(!turn_ended(Some((interrupted, at)), Some((at, wall))), "vor der Meldung gelesen");
        assert!(!turn_ended(Some((Tail { kind: TailKind::Open, at_ms: Some(wall + 800) }, later)), Some((at, wall))));
        assert!(!turn_ended(Some((Tail { kind: TailKind::Finished, at_ms: None }, later)), Some((at, wall))), "ohne Zeitstempel kein Beleg");
        assert!(!turn_ended(None, Some((at, wall))) && !turn_ended(Some((interrupted, later)), None));
        // Die Meldung kommt verspätet im Hub an (Laufzeit des Hook-Aufrufs): der Beleg darf etwas älter sein
        assert!(turn_ended(Some((Tail { kind: TailKind::Error, at_ms: Some(wall - 1500) }, later)), Some((at, wall))));
    }

    /// Der Rückfall schickt nie den nächsten Prompt der Warteschlange los (anders als ein echter Stop).
    #[test]
    fn the_fallback_never_advances_the_queue() {
        let mut e = claude_entry();
        feed(&mut e, "SessionStart");
        feed(&mut e, "UserPromptSubmit");
        e.meta.queue = vec!["weiter".into()];
        e.apply_fallback(ClaudeStatus::Idle);
        assert_eq!(e.meta.queue, vec!["weiter".to_string()]);
        assert_eq!(e.status, ClaudeStatus::Idle);
    }

    /// Mehrere Werkzeuge oder Unter-Agenten gleichzeitig: Die Meldung eines anderen Werkzeugs überschreibt die offene
    /// Freigabe-Frage nicht – vorher sprang der Status auf „arbeitet“ und blieb dort, während Claude wartete.
    #[test]
    fn an_open_approval_question_survives_events_of_other_tools() {
        let mut e = claude_entry();
        feed(&mut e, "SessionStart");
        feed(&mut e, "UserPromptSubmit");
        feed(&mut e, "PreToolUse");
        let ask = HookMsg { kind: Some("permission_prompt".into()), message: Some("Claude needs your permission to use Bash".into()), ..msg("Notification") };
        assert_eq!(e.take_hook(&ask).unwrap().new, ClaudeStatus::Permission);
        assert_eq!(e.track.perm_tool, "Bash");
        assert!(e.perm_since.is_some());
        // Ohne laufendes Terminal gilt „Eingabe seither“ – deshalb hier direkt die Regel prüfen
        let other = HookMsg { tool_name: Some("Read".into()), agent_id: Some("sub1".into()), ..msg("PostToolUse") };
        let none = ScreenHints::default();
        assert!(!hooks::tool_event_answers(&other, "Bash", false, none), "anderes Werkzeug, keine Eingabe");
        assert!(!hooks::tool_event_answers(&HookMsg { tool_name: Some("Bash".into()), ..msg("PreToolUse") }, "Bash", false, none), "ein Start beantwortet nichts");
        assert!(hooks::tool_event_answers(&HookMsg { tool_name: Some("Bash".into()), ..msg("PostToolUse") }, "Bash", false, none), "das gefragte Werkzeug ist fertig");
        assert!(hooks::tool_event_answers(&other, "Bash", true, none), "der Nutzer hat getippt");
        assert!(hooks::tool_event_answers(&other, "Bash", false, ScreenHints { spinner: true, dialog: false }), "die Arbeitsanzeige läuft wieder");
        let dialog = ScreenHints { dialog: true, ..none };
        assert!(!hooks::tool_event_answers(&other, "Bash", true, dialog), "die Rückfrage steht noch da");
        assert!(!hooks::tool_event_answers(&other, "Bash", false, ScreenHints { spinner: true, dialog: true }));
        assert!(!hooks::tool_event_answers(&HookMsg { tool_name: Some("Bash".into()), ..msg("PostToolUse") }, "Bash", false, dialog));
        // Ein abgeschickter Prompt oder ein Stop beenden die Frage immer
        assert_eq!(feed(&mut e, "Stop"), ClaudeStatus::Idle);
    }

    /// Claude wird abgeschossen (kein SessionEnd) und im selben Terminal neu gestartet: vorher blieben alle Meldungen des
    /// neuen Claude „fremd“ und der Status für immer stehen.
    #[test]
    fn a_killed_claude_is_noticed_at_the_shell_prompt_and_a_new_one_binds() {
        let mut e = claude_entry();
        assert!(!e.note_shell_seq(1), "erster Prompt: der Startbefehl läuft noch nicht");
        feed(&mut e, "SessionStart");
        assert_eq!(feed(&mut e, "UserPromptSubmit"), ClaudeStatus::Working);
        // Ein `claude -p` aus der Sitzung heraus bleibt fremd, solange Claude läuft (#260)
        let nested = HookMsg { claude_session: Some("N".into()), source: Some("startup".into()), ..msg("SessionStart") };
        assert!(e.take_hook(&nested).is_none());
        assert_eq!(e.hook_conv.bound.as_deref(), Some("A"));
        // Die Shell steht wieder am Prompt: Claude ist weg
        assert!(e.note_shell_seq(2));
        assert_eq!(e.status, ClaudeStatus::Unknown);
        assert!(!e.note_shell_seq(2), "derselbe Prompt zählt nur einmal");
        // Nachzügler des alten Claude zählen nicht
        assert!(e.take_hook(&msg("Stop")).is_none());
        assert_eq!(e.status, ClaudeStatus::Unknown);
        // `claude` von Hand neu gestartet: neue Konversation, wird zugeordnet
        let fresh = |ev: &str| HookMsg { claude_session: Some("B".into()), source: Some("startup".into()), ..msg(ev) };
        let f = e.take_hook(&fresh("SessionStart")).expect("neuer Claude wird angenommen");
        assert_eq!((f.new, f.conv), (ClaudeStatus::Idle, ConvMatch::Switch("B".into())));
        assert_eq!(e.take_hook(&fresh("UserPromptSubmit")).unwrap().new, ClaudeStatus::Working);
        // … und ab jetzt ist ein `claude -p` wieder fremd
        assert!(e.take_hook(&nested).is_none());
        assert_eq!(e.hook_conv.bound.as_deref(), Some("B"));
    }

    /// Der Prompt der Shell hat getäuscht (Claude angehalten, ungewöhnlicher Startbefehl): Meldet sich die zugeordnete
    /// Konversation wieder, gilt sie weiter – und ein `claude -p` übernimmt nichts.
    #[test]
    fn a_false_prompt_signal_heals_with_the_next_message() {
        let mut e = claude_entry();
        feed(&mut e, "SessionStart");
        feed(&mut e, "UserPromptSubmit");
        assert!(e.note_shell_seq(2));
        e.track.gone = e.track.gone.map(|(t, w)| (t - STRAGGLER - Duration::from_secs(1), w));
        assert_eq!(feed(&mut e, "PreToolUse"), ClaudeStatus::Working);
        assert!(!e.hook_conv.open);
        let nested = HookMsg { claude_session: Some("N".into()), source: Some("startup".into()), ..msg("SessionStart") };
        assert!(e.take_hook(&nested).is_none());
    }

    /// Claude startete gar nicht (falsche Option, nicht installiert): ein von Hand gestarteter Claude wird zugeordnet,
    /// obwohl die beim Start übergebene Konversation nie kam.
    #[test]
    fn a_manually_started_claude_binds_after_a_failed_launch() {
        let mut e = claude_entry();
        assert!(!e.note_shell_seq(1));
        assert!(e.note_shell_seq(2), "der Startbefehl ist zurück am Prompt");
        let fresh = HookMsg { claude_session: Some("B".into()), source: Some("startup".into()), ..msg("SessionStart") };
        assert_eq!(e.take_hook(&fresh).map(|f| f.conv), Some(ConvMatch::Bind("B".into())));
        // Ohne das Ende am Prompt bleibt eine fremde Konversation fremd (#260)
        let mut strict = claude_entry();
        assert!(strict.take_hook(&fresh).is_none());
    }

    /// Doppelt zugestellte Hooks: der zweite Stop zählt nicht – sonst liefe die Warteschlange zwei Prompts weiter.
    #[test]
    fn a_duplicated_stop_counts_once() {
        let mut e = claude_entry();
        feed(&mut e, "SessionStart");
        feed(&mut e, "UserPromptSubmit");
        assert_eq!(e.take_hook(&msg("Stop")).map(|f| (f.old, f.new)), Some((ClaudeStatus::Working, ClaudeStatus::Idle)));
        // Die Warteschlange hat inzwischen den nächsten Prompt geschickt
        e.assume_working();
        assert!(e.take_hook(&msg("Stop")).is_none(), "derselbe Stop noch einmal");
        assert_eq!(e.status, ClaudeStatus::Working);
        // Der Stop der nächsten Runde zählt wieder (dazwischen kam eine andere Meldung)
        feed(&mut e, "UserPromptSubmit");
        assert_eq!(feed(&mut e, "Stop"), ClaudeStatus::Idle);
        // … und auch ohne Meldung dazwischen, wenn er später kommt (Runde ohne Werkzeuge)
        e.track.last_stop = e.track.last_stop.map(|t| t - DUPLICATE_STOP - Duration::from_millis(100));
        e.assume_working();
        assert_eq!(feed(&mut e, "Stop"), ClaudeStatus::Idle);
    }

    #[test]
    fn launch_lines_count_the_commands_typed_at_start() {
        assert_eq!(launch_lines(None), 0);
        assert_eq!(launch_lines(Some("claude --session-id x\r")), 1);
        assert_eq!(launch_lines(Some("cd app\rclaude\r")), 2);
    }

    /// Ohne Hooks kommt der Status aus dem Transkript – nach dem Ende von Claude aber erst wieder, wenn es weitergeht.
    #[test]
    fn the_transcript_status_stops_when_claude_has_exited() {
        let mut e = claude_entry();
        let now = std::time::SystemTime::now();
        assert!(e.transcript_status_ok(Some(now)));
        e.note_shell_seq(2);
        assert!(!e.transcript_status_ok(Some(now - Duration::from_secs(5))), "seit dem Ende nichts geschrieben");
        assert!(!e.transcript_status_ok(None));
        assert!(e.transcript_status_ok(Some(now + Duration::from_secs(60))), "es wird wieder geschrieben");
        assert!(e.track.gone.is_none());
    }

    /// Vom Hub angenommenes „arbeitet“ (Prompt geschickt), das nichts bestätigt: zurück auf „bereit“ statt für immer.
    #[test]
    fn an_assumed_working_falls_back_to_ready() {
        let mut e = claude_entry();
        feed(&mut e, "SessionStart");
        e.assume_working();
        assert_eq!((e.status, e.track.turn.tentative), (ClaudeStatus::Working, Some(ClaudeStatus::Idle)));
        let o = |secs: u64| Observed { since_event: Duration::from_secs(secs), quiet: Duration::from_secs(secs), ..Observed::default() };
        assert_eq!(hooks::stale_status(e.status, &e.track.turn, &o(5)), None);
        assert_eq!(hooks::stale_status(e.status, &e.track.turn, &o(11)), Some(ClaudeStatus::Idle));
        // Bestätigt eine Meldung die Annahme, gilt die normale Frist
        feed(&mut e, "UserPromptSubmit");
        assert_eq!(e.track.turn.tentative, None);
        assert_eq!(hooks::stale_status(e.status, &e.track.turn, &o(11)), None);
        // Freigabe über den Knopf erteilt: bleibt die Bestätigung aus, lieber kein Status als wieder „Freigabe?“
        e.status = ClaudeStatus::Permission;
        e.assume_working();
        assert_eq!(e.track.turn.tentative, None);
    }
}
