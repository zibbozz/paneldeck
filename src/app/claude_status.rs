//! Status einer Claude-Sitzung über die Hook-Meldungen hinaus.
//!
//! Claude Code meldet nicht jedes Ende: kein `Stop` nach einer Unterbrechung (Esc), nach einem API-Fehler oder einer im
//! Terminal abgelehnten Freigabe; keine Meldung, wenn der Prozess abgeschossen wird; nach einer im Terminal erteilten
//! Freigabe erst wieder, wenn das Werkzeug fertig ist. Ohne Gegenprobe bliebe „arbeitet“ bzw. „Freigabe?“ stehen, bis
//! der nächste Prompt durch ist. Hier laufen deshalb zusammen:
//!
//! * die Annahme einer Hook-Meldung für eine Sitzung ([`Entry::take_hook`]): Schlüssel und Konversation (#260), Status,
//!   und die Regeln, wann eine offene Rückfrage endet und wann die Warteschlange weiterlaufen darf;
//! * der Rückfall je Sitzung ([`App::claude_status_tick`] → [`crate::hooks::stale_status`]) aus Transkript
//!   (`usage::Usage::ended_after`), Ruhe des Terminals und Bildschirm;
//! * das Ende von Claude am Prompt der Shell (Shell-Integration).
//!
//! **Der Prompt der Shell ist kein Beweis** (Review #282): OSC 133;A bzw. OSC 9;9 kann jedes Programm ausgeben, das im
//! Terminal läuft – auch eines, das Claude gestartet hat. Als Ende gilt er nur, wenn der Hub danach sieht, dass im
//! Vordergrund der Shell nichts mehr läuft (`platform::process::shell_is_idle`, [`Entry::prompt_checked`]). Erst dann fällt der
//! Status weg (auch eine offene Freigabe-Frage – die schützt vor Eingaben von Agenten), werden Nachzügler verworfen und
//! darf ein SessionStart einer neuen Konversation übernehmen. Wo sich das nicht prüfen lässt (WSL: Linux-Prozesse sieht
//! Windows nicht), ändert das Signal nur „bereit“/„arbeitet“ auf unbekannt ([`Entry::prompt_unconfirmed`]).

use super::*;
use crate::hooks::{ConvMatch, Observed, ScreenHints, TurnState};

/// So lange nach dem bestätigten Ende zählen Meldungen des beendeten Claude noch als Nachzügler (verworfen).
const STRAGGLER: Duration = Duration::from_secs(3);
/// Abstand der Gegenprobe (Bildschirm lesen, Transkript-Stand holen).
const CHECK_EVERY: Duration = Duration::from_millis(500);
/// Zeitpunkte nach einem Prompt-Signal (ms), zu denen der Hub nachsieht, ob im Vordergrund der Shell noch etwas läuft.
/// Mehrmals: Der Prompt selbst startet oft kurz Hilfsprogramme (git, oh-my-posh), die noch an der Konsole hängen.
const PROMPT_TRIES_MS: [u64; 5] = [0, 400, 1000, 2000, 3500];
/// Mindestabstand zweier solcher Abfragen je Sitzung (ein Programm kann das Signal beliebig oft ausgeben).
const PROMPT_TRY_GAP: Duration = Duration::from_millis(300);
/// So lange muss die Shell nach einem Prompt frei sein, damit ein Startbefehl als gescheitert gilt, von dessen Claude
/// noch keine Meldung kam: Der Befehl wird beim ersten Prompt erst gelesen.
const NEVER_STARTED: Duration = Duration::from_secs(3);

/// Der Claude, den der Hub beim Start der Sitzung aufgerufen hat.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Launched {
    /// Noch keine Meldung von ihm.
    #[default]
    Expected,
    /// Er läuft und ist der, dessen Meldungen zählen: Die Warteschlange darf von selbst weiterlaufen.
    Running,
    /// Er ist beendet (SessionEnd, bestätigtes Ende am Prompt) oder gar nicht gestartet. Was danach in diesem Terminal
    /// als Claude läuft (von Hand gestartet, ein `claude -p`), bekommt nie von selbst einen Prompt der Warteschlange –
    /// ein `claude -p` nähme ihn nicht an, und die Shell führte ihn danach als Befehl aus.
    Ended,
}

/// Die Shell stand wieder an ihrem Prompt.
#[derive(Clone, Copy)]
pub(super) struct Gone {
    at: Instant,
    wall: std::time::SystemTime,
    /// Der Hub hat gesehen, dass im Vordergrund der Shell nichts mehr läuft.
    confirmed: bool,
}

/// Was der Hub je Start über den Claude einer Sitzung mitführt.
#[derive(Default)]
pub(super) struct Track {
    pub(super) turn: TurnState,
    /// Letzte angenommene Hook-Meldung bzw. vom Hub selbst gesetzter Status.
    pub(super) evt: Option<Instant>,
    /// Marke im Transkript für diese letzte Meldung (`UsageWatcher::mark`): Nur was dahinter steht, belegt etwas.
    pub(super) mark_seq: u64,
    /// Die Marke ist dem Beobachter noch nicht gemeldet (`App::claude_status_tick`).
    pub(super) mark_pending: bool,
    /// `Session::input_stamp` bei der letzten Meldung: Eingaben danach können Claude unterbrochen oder eine Rückfrage
    /// beantwortet haben.
    pub(super) input_base: u64,
    /// Werkzeug, für das die offene Freigabe-Frage gestellt wurde (aus der Benachrichtigung), sonst leer.
    pub(super) perm_tool: String,
    /// Zeilen des Startbefehls (Startbefehl des Profils, `claude …`): So viele Prompts zeigt die Shell, bevor der
    /// letzte davon läuft. Nur eine zusätzliche Hürde für „Claude ist gar nicht gestartet“ – Themes mit doppeltem
    /// Prompt zählen anders, entscheidend bleibt, was an der Konsole hängt.
    pub(super) launch_lines: u64,
    /// Zuletzt gesehener Prompt-Zähler der Shell.
    pub(super) shell_seq: u64,
    /// Ein Prompt-Signal wartet auf die Bestätigung über die Prozesse der Shell: seit wann, wie viele Abfragen schon.
    pub(super) prompt_check: Option<(Instant, usize)>,
    pub(super) last_prompt_try: Option<Instant>,
    /// Die Shell stand nach dem Start wieder an ihrem Prompt.
    pub(super) gone: Option<Gone>,
    /// Die letzte angenommene Meldung war ein Stop (die Runde ist zu; ein weiterer Stop ohne Meldung dazwischen ist
    /// derselbe noch einmal).
    pub(super) stop_last: bool,
    /// Lokale Shell, deren Vordergrundprogramm der Hub sehen kann (PowerShell, cmd, Git Bash – nicht WSL).
    pub(super) confirmable: bool,
    /// In diesem Start kam schon eine Meldung eines Claude, der seither nicht beendet wurde.
    pub(super) claude_seen: bool,
    pub(super) launched: Launched,
    /// Der Hub hat die Warteschlange angehalten, weil Claude beendet ist (beim nächsten Start läuft sie wieder).
    pub(super) auto_paused: bool,
}

/// Nächste Nummer einer Marke im Transkript (aufsteigend über alle Sitzungen).
fn next_mark() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Zeilen eines Startbefehls (jede wird mit Enter abgeschickt).
pub(super) fn launch_lines(initial: Option<&str>) -> u64 {
    initial.map_or(0, |i| i.bytes().filter(|b| *b == b'\r').count() as u64)
}

/// Prompt-Zähler der Shell: OSC 133;A (PowerShell, bash), sonst die Ordnermeldung, die cmd mit jedem Prompt schickt.
fn shell_seq(st: &crate::terminal::ShellState) -> u64 {
    if st.prompt_seq > 0 { st.prompt_seq } else { st.cwd_seq }
}

/// Ergebnis einer angenommenen Hook-Meldung.
pub(super) struct HookEffect {
    pub(super) old: ClaudeStatus,
    pub(super) new: ClaudeStatus,
    pub(super) conv: ConvMatch,
    /// Meldung ohne Schlüssel (globale Hooks einer älteren paneldeck.exe): nur für den Status.
    pub(super) keyless: bool,
    /// Die Warteschlange darf jetzt den nächsten Prompt senden: der vom Hub gestartete Claude meldet sich bereit, oder
    /// eine Runde, die mit einem abgeschickten Prompt begann, ist zu Ende.
    pub(super) advance: bool,
    /// Claude ist beendet und die Warteschlange wurde deshalb angehalten.
    pub(super) paused: bool,
}

impl Entry {
    /// Eine Meldung oder ein vom Hub gesetzter Status: ab jetzt zählt die Zeit für den Rückfall, und nur was ab jetzt
    /// im Transkript erscheint, belegt etwas.
    pub(super) fn stamp_event(&mut self) {
        self.track.evt = Some(Instant::now());
        self.track.mark_seq = next_mark();
        self.track.mark_pending = true;
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

    /// Hook-Meldung für diese Sitzung annehmen ([`Entry::take_hook_with`]); Eingaben und Bildschirm kommen vom Terminal.
    pub(super) fn take_hook(&mut self, m: &HookMsg) -> Option<HookEffect> {
        // Den Bildschirm braucht nur eine offene Rückfrage.
        let asking = matches!(self.status, ClaudeStatus::Permission | ClaudeStatus::Input);
        let (input_since, screen) = match &self.live {
            Some(s) => (s.input_stamp() > self.track.input_base, asking.then(|| hooks::screen_hints(&s.screen_text()))),
            None => (true, None),
        };
        self.take_hook_with(m, input_since, || screen.unwrap_or_default())
    }

    /// Hook-Meldung annehmen: Schlüssel und Konversation prüfen (#260), Status fortschreiben. `None` = verworfen.
    /// Transkript, Ordner und Benachrichtigungen macht der Aufrufer (`App::poll_hooks`). `input_since` = seit der
    /// letzten Meldung wurde ins Terminal getippt; `screen` wertet den Bildschirm aus (nur bei offener Rückfrage).
    pub(super) fn take_hook_with(&mut self, m: &HookMsg, input_since: bool, screen: impl FnOnce() -> ScreenHints) -> Option<HookEffect> {
        // Nachzügler des beendeten Claude (z. B. ein Stop, das erst nach dem Prompt der Shell ankommt) zählen nicht;
        // ein neuer Claude meldet sich zuerst mit SessionStart. Nur nach einem bestätigten Ende – ein vorgetäuschter
        // Prompt darf keine Meldung der laufenden Konversation kosten.
        if m.event != "SessionStart" && self.track.gone.is_some_and(|g| g.confirmed && g.at.elapsed() < STRAGGLER) {
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
        let expected = self.hook_conv.expect.clone();
        let conv = if keyless { ConvMatch::NoId } else { self.hook_conv.feed(m) };
        if conv == ConvMatch::Foreign {
            return None;
        }
        // Der Startbefehl galt als gescheitert, aber sein Claude meldet sich doch (mit der beim Start übergebenen
        // Konversation, die nur er kennt): Er ist der gestartete – eine deshalb angehaltene Warteschlange läuft wieder.
        if self.track.launched == Launched::Ended && matches!(&conv, ConvMatch::Bind(id) if expected.as_deref() == Some(id.as_str())) {
            self.track.launched = Launched::Expected;
            if std::mem::take(&mut self.track.auto_paused) {
                self.meta.queue_paused = false;
            }
        }
        // Derselbe Stop doppelt zugestellt (die Hooks stehen zweimal in den Einstellungen und Claude legt sie nicht
        // zusammen): Je Runde zählt einer – ohne Meldung dazwischen hat keine neue Runde begonnen.
        if m.event == "Stop" && self.track.stop_last {
            return None;
        }
        self.track.stop_last = m.event == "Stop";
        let old = self.status;
        self.hook_seen = true;
        self.track.claude_seen = true;
        if m.event == "SessionStart" {
            self.track.gone = None;
        }
        let before = self.track.turn.clone();
        let mut new = hooks::status_after(old, &mut self.track.turn, m);
        // Offene Rückfrage („Freigabe?“/„Eingabe“): Claude meldet sie kein zweites Mal – sie darf nicht verloren gehen,
        // solange sie offen ist.
        let asking = |s: ClaudeStatus| matches!(s, ClaudeStatus::Permission | ClaudeStatus::Input);
        if asking(old) {
            if new == ClaudeStatus::Working && matches!(m.event.as_str(), "PreToolUse" | "PostToolUse") {
                // Die Meldung eines anderen, gleichzeitig laufenden Werkzeugs oder Unter-Agenten beendet sie nicht.
                if !hooks::tool_event_answers(m, &self.track.perm_tool, input_since, screen()) {
                    new = old;
                }
            } else if new == ClaudeStatus::Idle && m.event == "Notification" && screen().dialog {
                // „Claude wartet auf deine Eingabe“ (idle_prompt), während die Rückfrage noch zu sehen ist: Sie bleibt –
                // „bereit“ ließe Agenten und die Warteschlange in die Rückfrage tippen.
                new = old;
                self.track.turn = before.clone();
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
        // Warteschlange: von selbst nur für den Claude, den der Hub gestartet hat – wenn er sich nach dem Start bereit
        // meldet (Start-Prompt) und am Ende einer Runde, die mit einem abgeschickten Prompt begann. Nie nach einem
        // bloßen SessionStart (`/clear`, ein neu gebundener Claude), einer Benachrichtigung oder einem SubagentStop.
        let (mut advance, mut paused) = (false, false);
        if hooks::session_end_is_exit(m) {
            self.track.claude_seen = false;
            paused = self.end_launch();
        } else {
            match self.track.launched {
                Launched::Expected => {
                    self.track.launched = Launched::Running;
                    advance = m.event == "SessionStart" && new == ClaudeStatus::Idle;
                }
                Launched::Running => advance = m.event == "Stop" && before.prompted && new == ClaudeStatus::Idle,
                Launched::Ended => {}
            }
        }
        Some(HookEffect { old, new, conv, keyless, advance, paused })
    }

    /// Konversation der Sitzung nach einer angenommenen Meldung mit Schlüssel fortschreiben (`conv` aus
    /// [`Entry::take_hook`]). Gespeichert – und später mit `--resume` fortgesetzt – wird nur eine Konversation, die der
    /// Sitzung gehört (`ConvBinding::own`, #302): die beim Start übergebene, die mit `--continue` fortgesetzte oder eine,
    /// zu der der vom Hub gestartete Claude selbst gewechselt hat. Ein von Hand im selben Terminal gestarteter Claude
    /// oder ein anderer Prozess mit dem Schlüssel der Sitzung ändert sie nie, auch nicht „Inhalt vorhanden“ oder das
    /// Transkript (Kosten, MCP). Meldungen ohne Schlüssel (`keyless`) nie. `true`, wenn sich das Transkript geändert hat.
    pub(super) fn note_conversation(&mut self, m: &HookMsg, conv: &ConvMatch, keyless: bool) -> bool {
        if keyless || !self.hook_conv.own {
            return false;
        }
        let mut transcript = false;
        if let Some(tp) = m.transcript_path.as_ref().filter(|p| !p.is_empty()) {
            if self.meta.transcript != *tp {
                self.meta.transcript = tp.clone();
                transcript = true;
            }
        }
        if !self.is_claude() {
            return transcript;
        }
        if let ConvMatch::Bind(cs) | ConvMatch::Switch(cs) = conv {
            if self.meta.claude_session != *cs {
                self.meta.claude_session = cs.clone();
                // Eine andere Konversation als beim Start angegeben: Inhalt hat sie nur, wenn sie fortgesetzt wurde.
                self.meta.claude_started = false;
            }
        }
        match (m.event.as_str(), m.source.as_deref()) {
            // Fortgesetzte Konversation (`--resume`, `--continue`, `/resume`): sie hat Inhalt.
            ("SessionStart", Some("resume")) | ("UserPromptSubmit", _) => self.meta.claude_started = true,
            // `/clear`: neue, leere Konversation.
            ("SessionStart", Some("clear")) => self.meta.claude_started = false,
            _ => {}
        }
        transcript
    }

    /// Der vom Hub gestartete Claude ist beendet (oder nie gestartet): Die Warteschlange läuft in diesem Start nicht
    /// mehr von selbst. Warten noch Prompts, wird sie angehalten (`true`) – sichtbar im Fenster der Warteschlange, bis
    /// der Nutzer sie fortsetzt oder die Sitzung neu lädt.
    fn end_launch(&mut self) -> bool {
        self.track.launched = Launched::Ended;
        if self.is_claude() && !self.meta.queue.is_empty() && !self.meta.queue_paused {
            self.meta.queue_paused = true;
            self.track.auto_paused = true;
            return true;
        }
        false
    }

    /// Bestätigtes Ende: Die Shell zeigt ihren Prompt und in ihrem Vordergrund läuft nichts mehr – Claude ist beendet,
    /// auch ohne SessionEnd (abgeschossen, abgestürzt, Meldung verloren). Der Status fällt weg (auch eine offene
    /// Rückfrage: es gibt sie nicht mehr), ein neu gestarteter Claude darf sich zuordnen. `true`: Warteschlange angehalten.
    pub(super) fn claude_ended(&mut self) -> bool {
        self.hook_conv.shell_prompt();
        self.track.gone = Some(Gone { at: Instant::now(), wall: std::time::SystemTime::now(), confirmed: true });
        self.track.turn = TurnState::default();
        self.track.claude_seen = false;
        self.track.stop_last = false;
        if self.status != ClaudeStatus::Unknown {
            self.status = ClaudeStatus::Unknown;
            self.status_msg.clear();
            self.perm_since = None;
        }
        self.end_launch()
    }

    /// Prompt der Shell ohne Bestätigung (WSL und andere Shells, deren Vordergrund der Hub nicht sieht). Das Signal kann
    /// vorgetäuscht sein: Eine offene Rückfrage bleibt, keine Meldung wird verworfen, niemand darf neu übernehmen. Nur
    /// „bereit“/„arbeitet“ fällt weg (ohne Folgen: unbekannt löst nichts aus) und der Status aus dem Transkript ruht,
    /// bis wieder hineingeschrieben wird. `true`, wenn sich der Status geändert hat.
    pub(super) fn prompt_unconfirmed(&mut self) -> bool {
        self.track.gone = Some(Gone { at: Instant::now(), wall: std::time::SystemTime::now(), confirmed: false });
        let derived = self.is_claude() && !self.hook_seen;
        if (self.track.claude_seen || derived) && matches!(self.status, ClaudeStatus::Idle | ClaudeStatus::Working) {
            self.status = ClaudeStatus::Unknown;
            self.status_msg.clear();
            self.track.turn.tentative = None;
            self.track.turn.stale = false;
            return true;
        }
        false
    }

    /// Neuen Prompt-Zähler der Shell vermerken; `true` bei einem neuen Prompt-Signal.
    pub(super) fn note_shell_seq(&mut self, seq: u64) -> bool {
        let new = seq > self.track.shell_seq;
        self.track.shell_seq = self.track.shell_seq.max(seq);
        new
    }

    /// Ergebnis einer Abfrage nach einem Prompt-Signal: `idle` = im Vordergrund der Shell läuft nichts
    /// (`platform::process::shell_is_idle`; `None` = nicht zu ermitteln). `Some(angehalten)`, wenn damit ein Ende feststeht.
    pub(super) fn prompt_checked(&mut self, idle: Option<bool>) -> Option<bool> {
        let (since, tries) = self.track.prompt_check?;
        self.track.last_prompt_try = Some(Instant::now());
        if idle == Some(true) {
            // Status ohne Hooks (aus dem Transkript): auch der gehört einem Claude, der lief.
            let derived = self.is_claude() && !self.hook_seen && self.status != ClaudeStatus::Unknown;
            if self.track.claude_seen || derived {
                self.track.prompt_check = None;
                return Some(self.claude_ended());
            }
            // Vom gestarteten Claude kam nie eine Meldung, der Startbefehl ist durch (mehr Prompts als seine Zeilen) und
            // die Shell ist seit dem Prompt frei: Er ist gescheitert (nicht installiert, falsche Option). Ein von Hand
            // gestarteter Claude darf sich zuordnen. Die Prompts allein reichen nicht (ein Programm kann sie ausgeben),
            // die freie Shell allein auch nicht (ein Startbefehl des Profils kann in der Shell selbst laufen).
            if self.wants_launch_check() && since.elapsed() >= NEVER_STARTED {
                self.track.prompt_check = None;
                self.hook_conv.shell_prompt();
                return Some(self.end_launch());
            }
        }
        self.track.prompt_check = (tries + 1 < PROMPT_TRIES_MS.len()).then_some((since, tries + 1));
        None
    }

    /// Wartet der Hub noch auf den Claude seines Startbefehls, und ist dieser Befehl laut Prompt-Zähler schon durch?
    fn wants_launch_check(&self) -> bool {
        self.is_claude() && self.track.launched == Launched::Expected && self.track.shell_seq > self.track.launch_lines
    }

    /// Gibt es an einem Prompt der Shell etwas zu entscheiden? Sonst fragt der Hub die Konsole gar nicht erst ab.
    pub(super) fn prompt_matters(&self) -> bool {
        self.track.claude_seen || (self.is_claude() && !self.hook_seen && self.status != ClaudeStatus::Unknown) || self.wants_launch_check()
    }

    /// Ist die nächste Abfrage nach einem Prompt-Signal fällig?
    fn prompt_try_due(&self) -> bool {
        self.track.prompt_check.is_some_and(|(since, tries)| since.elapsed() >= Duration::from_millis(PROMPT_TRIES_MS[tries.min(PROMPT_TRIES_MS.len() - 1)]))
            && self.track.last_prompt_try.is_none_or(|t| t.elapsed() >= PROMPT_TRY_GAP)
    }

    /// Zählt das Transkript für den Status ohne Hooks? Nach einem Prompt der Shell erst wieder, wenn danach
    /// hineingeschrieben wurde (Claude läuft noch oder wieder).
    pub(super) fn transcript_status_ok(&mut self, modified: Option<std::time::SystemTime>) -> bool {
        match self.track.gone {
            Some(g) if modified.is_none_or(|m| m <= g.wall) => false,
            Some(_) => {
                self.track.gone = None;
                true
            }
            None => true,
        }
    }

    /// Beobachtungen für den Rückfall; `turn_ended` = das Transkript belegt das Ende der Runde nach der letzten
    /// Meldung. Den Bildschirm liest das nur, wenn es darauf ankommen kann – mit einem Beleg immer.
    fn observe(&self, turn_ended: bool) -> Option<Observed> {
        let s = self.live.as_ref()?;
        let since_event = self.track.evt.map_or(Duration::MAX, |t| t.elapsed());
        let quiet = s.quiet_for();
        let stamp = s.input_stamp();
        let input_since = (stamp > self.track.input_base).then(|| Duration::from_millis(crate::terminal::mono_ms().saturating_sub(stamp)));
        let need_screen = turn_ended
            || match self.status {
                ClaudeStatus::Working => since_event >= hooks::STALE_SETTLE && quiet >= hooks::STALE_SETTLE,
                ClaudeStatus::Permission | ClaudeStatus::Input => input_since.is_some(),
                ClaudeStatus::Unknown | ClaudeStatus::Idle => false,
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
            self.track.turn.prompted = false;
        }
    }

    /// Gegenprobe für eine Sitzung; `true`, wenn sich der Status geändert hat.
    fn stale_tick(&mut self, turn_ended: bool) -> bool {
        let Some(o) = self.observe(turn_ended) else { return false };
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
    /// Hinweis, dass die Warteschlange einer Sitzung angehalten wurde, weil Claude beendet ist.
    pub(super) fn queue_paused_note(&self, id: u64) -> Option<String> {
        let e = self.entries.get(&id)?;
        let (name, n) = (e.display_name(), e.meta.queue.len());
        let lang = self.settings.language;
        Some(match lang {
            Lang::De => format!("Claude in {} ist beendet – die Warteschlange ({n}) ist angehalten. Weiter geht es im Fenster der Warteschlange mit „Fortsetzen“ oder mit „Neu laden“.", lang.q(&name)),
            Lang::En => format!("Claude in {} has ended – its queue ({n}) is paused. Continue with “Resume” in the queue window or with “Reload”.", lang.q(&name)),
        })
    }

    /// Je Bild (Hauptdurchgang): Prompt-Signale der Shell prüfen (Ende von Claude), Marken an den Beobachter der
    /// Transkripte melden und, in größeren Abständen, den Status gegenprüfen, wenn Hook-Meldungen ausbleiben.
    pub(super) fn claude_status_tick(&mut self, ctx: &egui::Context) {
        let check = self.last_status_check.elapsed() >= CHECK_EVERY;
        if check {
            self.last_status_check = Instant::now();
        }
        let mut changed = false;
        let mut paused: Vec<u64> = Vec::new();
        for (id, e) in self.entries.iter_mut() {
            if std::mem::take(&mut e.track.mark_pending) {
                self.usage.mark(*id, e.track.mark_seq);
            }
            let Some(s) = e.live.as_ref() else { continue };
            let seq = shell_seq(&s.shell_state.lock().unwrap_or_else(|x| x.into_inner()));
            let (exited, shell_pid) = (s.has_exited(), s.shell_pid());
            // Ein Prompt der Shell zählt nur für Sitzungen, in denen Claude eine Rolle spielt.
            if e.note_shell_seq(seq) && !exited && (e.is_claude() || e.hook_seen) {
                if e.track.confirmable && shell_pid.is_some() {
                    if e.prompt_matters() {
                        e.track.prompt_check = Some((Instant::now(), 0));
                    }
                } else {
                    changed |= e.prompt_unconfirmed();
                }
            }
            if let Some(pid) = shell_pid.filter(|_| !exited && e.prompt_try_due()) {
                if let Some(was_paused) = e.prompt_checked(crate::platform::process::shell_is_idle(pid)) {
                    changed = true;
                    if was_paused {
                        paused.push(*id);
                    }
                }
            }
            // Nur mit Hook-Meldungen: ohne sie kommt der Status aus dem Transkript (`sync_watchers`).
            let stale = match e.status {
                ClaudeStatus::Working | ClaudeStatus::Permission | ClaudeStatus::Input => true,
                ClaudeStatus::Unknown => e.track.turn.stale,
                ClaudeStatus::Idle => false,
            };
            if check && stale && e.hook_seen && !exited {
                changed |= e.stale_tick(self.usage.ended_after(*id, e.track.mark_seq));
            }
        }
        for id in paused {
            if let Some(n) = self.queue_paused_note(id) {
                self.notify_text(n);
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

    fn claude_entry() -> Entry {
        let mut e = Entry::new(SessionMeta { id: 1, claude: true, ..SessionMeta::default() });
        e.session_key = "k".into();
        e.hook_conv = hooks::ConvBinding::launch(Some("A".into()), false);
        e.track.confirmable = true;
        e.track.launch_lines = 1;
        e
    }

    fn msg(event: &str) -> HookMsg {
        HookMsg { session: 1, event: event.into(), claude_session: Some("A".into()), key: Some("k".into()), ..HookMsg::default() }
    }

    fn feed(e: &mut Entry, event: &str) -> ClaudeStatus {
        e.take_hook(&msg(event)).map(|f| f.new).unwrap_or(e.status)
    }

    const NO_SCREEN: ScreenHints = ScreenHints { spinner: false, dialog: false };
    const DIALOG: ScreenHints = ScreenHints { spinner: false, dialog: true };

    /// Der gemeldete Fehler: Nach Esc kommt kein Stop – ohne Rückfall bliebe „arbeitet“ stehen.
    #[test]
    fn an_interrupted_turn_leaves_working_with_evidence_from_the_transcript() {
        let mut e = claude_entry();
        feed(&mut e, "SessionStart");
        assert_eq!(feed(&mut e, "UserPromptSubmit"), ClaudeStatus::Working);
        let first = e.track.mark_seq;
        assert_eq!(feed(&mut e, "PreToolUse"), ClaudeStatus::Working);
        let mark = e.track.mark_seq;
        assert!(mark > first && e.track.mark_pending, "jede Meldung setzt eine neue Marke im Transkript");
        let calm = Observed { since_event: Duration::from_secs(5), quiet: Duration::from_secs(5), ..Observed::default() };
        // Ohne Beleg und ohne lange Ruhe bleibt es bei „arbeitet“ (ein langes Werkzeug meldet zwischendurch nichts)
        assert_eq!(hooks::stale_status(e.status, &e.track.turn, &calm), None);
        // Beleg: die Unterbrechung steht hinter der Marke der letzten Meldung im Transkript
        let o = Observed { turn_ended: true, ..calm };
        assert_eq!(hooks::stale_status(e.status, &e.track.turn, &o), Some(ClaudeStatus::Idle));
        e.apply_fallback(ClaudeStatus::Idle);
        assert_eq!(e.status, ClaudeStatus::Idle);
        assert!(!e.track.turn.open && !e.track.turn.prompted, "die Runde ist zu");
        assert_eq!(e.track.mark_seq, mark, "die letzte Meldung bleibt der Bezug für Belege");
        // Ohne Beleg erst unbekannt; kommt er später doch, wird daraus bereit
        feed(&mut e, "UserPromptSubmit");
        let mark = e.track.mark_seq;
        e.apply_fallback(ClaudeStatus::Unknown);
        assert!(e.track.turn.stale);
        assert_eq!(e.track.mark_seq, mark);
        assert_eq!(hooks::stale_status(e.status, &e.track.turn, &o), Some(ClaudeStatus::Idle));
        // Nach einem bestätigten Ende von Claude gilt das nicht mehr
        e.claude_ended();
        assert_eq!(hooks::stale_status(e.status, &e.track.turn, &o), None);
    }

    /// Review #282 (MEDIUM-3): Auch mit Beleg aus dem Transkript nie „bereit“, solange eine Rückfrage zu sehen ist.
    #[test]
    fn evidence_never_ends_a_question_that_is_still_on_screen() {
        let turn = TurnState::default();
        let o = Observed { since_event: Duration::from_secs(5), quiet: Duration::from_secs(5), turn_ended: true, screen: DIALOG, ..Observed::default() };
        for cur in [ClaudeStatus::Permission, ClaudeStatus::Input, ClaudeStatus::Working] {
            assert_eq!(hooks::stale_status(cur, &turn, &o), None, "{cur:?}");
            assert_eq!(hooks::stale_status(cur, &turn, &Observed { screen: NO_SCREEN, ..o }), Some(ClaudeStatus::Idle), "{cur:?}");
        }
        let stale = TurnState { stale: true, ..TurnState::default() };
        assert_eq!(hooks::stale_status(ClaudeStatus::Unknown, &stale, &o), None);
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
        // Auch ein später Stop derselben Runde nicht: sie gilt nicht mehr als mit einem Prompt begonnen
        assert!(!e.take_hook(&msg("Stop")).unwrap().advance);
    }

    /// Wann die Warteschlange von selbst weiterläuft (Review #282, MEDIUM-2): nur für den vom Hub gestarteten Claude,
    /// wenn er sich nach dem Start bereit meldet und am Ende einer Runde, die mit einem abgeschickten Prompt begann.
    #[test]
    fn the_queue_advances_only_for_the_launched_claude_after_a_prompted_turn() {
        let mut e = claude_entry();
        let adv = |e: &mut Entry, m: HookMsg| e.take_hook(&m).map(|f| f.advance);
        assert_eq!(adv(&mut e, HookMsg { source: Some("startup".into()), ..msg("SessionStart") }), Some(true), "Start-Prompt");
        assert_eq!(e.track.launched, Launched::Running);
        assert_eq!(adv(&mut e, msg("UserPromptSubmit")), Some(false));
        assert_eq!(adv(&mut e, msg("PostToolUse")), Some(false));
        assert_eq!(adv(&mut e, msg("Stop")), Some(true), "Runde mit Prompt zu Ende");
        // Runde ohne Prompt (von einem Hintergrund-Ergebnis geweckt), SubagentStop, idle_prompt, /clear: nie
        assert_eq!(adv(&mut e, msg("PreToolUse")), Some(false));
        assert_eq!(adv(&mut e, msg("Stop")), Some(false));
        assert_eq!(adv(&mut e, HookMsg { agent_id: Some("a".into()), ..msg("PostToolUse") }), Some(false));
        assert_eq!(adv(&mut e, msg("SubagentStop")), Some(false));
        assert_eq!(e.status, ClaudeStatus::Idle);
        feed(&mut e, "UserPromptSubmit");
        assert_eq!(adv(&mut e, HookMsg { kind: Some("idle_prompt".into()), ..msg("Notification") }), Some(false));
        let clear = |ev: &str, id: &str| HookMsg { claude_session: Some(id.into()), source: Some("clear".into()), reason: Some("clear".into()), ..msg(ev) };
        e.meta.queue = vec!["eins".into()];
        let end = e.take_hook(&clear("SessionEnd", "A")).unwrap();
        assert!(!end.paused && !e.meta.queue_paused, "/clear beendet Claude nicht");
        assert_eq!(adv(&mut e, clear("SessionStart", "C")), Some(false));
        assert_eq!(e.track.launched, Launched::Running);
        // … danach wieder am Ende einer Runde mit Prompt
        let c = |ev: &str| HookMsg { claude_session: Some("C".into()), ..msg(ev) };
        assert_eq!(adv(&mut e, c("UserPromptSubmit")), Some(false));
        assert_eq!(adv(&mut e, c("Stop")), Some(true));
        // Claude beendet (/exit): Warteschlange angehalten; ein danach gebundener Claude (z. B. `claude -p`) bekommt
        // nie von selbst einen Prompt – die Shell führte ihn nach dessen Ende als Befehl aus
        let exit = e.take_hook(&HookMsg { reason: Some("prompt_input_exit".into()), ..c("SessionEnd") }).unwrap();
        assert!(exit.paused && e.meta.queue_paused && e.track.auto_paused);
        assert_eq!((e.status, e.track.launched), (ClaudeStatus::Unknown, Launched::Ended));
        let p = |ev: &str| HookMsg { claude_session: Some("P".into()), source: Some("startup".into()), ..msg(ev) };
        assert_eq!(adv(&mut e, p("SessionStart")), Some(false), "neu gebunden: kein Start-Prompt");
        assert_eq!(adv(&mut e, p("UserPromptSubmit")), Some(false));
        assert_eq!(adv(&mut e, p("Stop")), Some(false), "auch nicht am Ende seiner Runde");
        // Ohne wartende Prompts wird nichts angehalten
        let mut e = claude_entry();
        feed(&mut e, "SessionStart");
        assert!(!e.take_hook(&msg("SessionEnd")).unwrap().paused);
        assert!(!e.meta.queue_paused && e.track.launched == Launched::Ended);
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
        let other = HookMsg { tool_name: Some("Read".into()), agent_id: Some("sub1".into()), ..msg("PostToolUse") };
        let take = |e: &mut Entry, m: &HookMsg, input: bool, screen: ScreenHints| e.take_hook_with(m, input, || screen).map(|f| f.new);
        // Anderes Werkzeug, keine Eingabe seither: die Frage bleibt
        assert_eq!(take(&mut e, &other, false, NO_SCREEN), Some(ClaudeStatus::Permission));
        assert_eq!(take(&mut e, &HookMsg { tool_name: Some("Bash".into()), ..msg("PreToolUse") }, false, NO_SCREEN), Some(ClaudeStatus::Permission), "ein Start beantwortet nichts");
        // Der Nutzer hat getippt, aber die (nächste) Rückfrage steht noch da: bleibt
        assert_eq!(take(&mut e, &other, true, DIALOG), Some(ClaudeStatus::Permission));
        // Die Arbeitsanzeige läuft wieder (Antwort von anderswo): beantwortet
        let mut f = claude_entry();
        f.status = ClaudeStatus::Permission;
        assert_eq!(take(&mut f, &other, false, ScreenHints { spinner: true, dialog: false }), Some(ClaudeStatus::Working));
        // Das gefragte Werkzeug ist fertig: beantwortet
        assert_eq!(take(&mut e, &HookMsg { tool_name: Some("Bash".into()), ..msg("PostToolUse") }, false, NO_SCREEN), Some(ClaudeStatus::Working));
        // Ein abgeschickter Prompt oder ein Stop beenden die Frage immer
        e.take_hook(&ask);
        assert_eq!(feed(&mut e, "Stop"), ClaudeStatus::Idle);
    }

    /// Review #282 (MEDIUM-4): „Claude wartet auf deine Eingabe“ (idle_prompt) macht aus einer offenen Rückfrage kein
    /// „bereit“, solange sie zu sehen ist – und schickt nie einen Prompt der Warteschlange hinein.
    #[test]
    fn an_idle_notification_does_not_end_a_visible_question() {
        let mut e = claude_entry();
        feed(&mut e, "SessionStart");
        feed(&mut e, "UserPromptSubmit");
        let ask = HookMsg { kind: Some("permission_prompt".into()), message: Some("Claude needs your permission to use Bash".into()), ..msg("Notification") };
        e.take_hook(&ask);
        e.meta.queue = vec!["nicht in den Dialog".into()];
        for idle in [HookMsg { kind: Some("idle_prompt".into()), ..msg("Notification") }, HookMsg { message: Some("Claude is waiting for your input".into()), ..msg("Notification") }] {
            let f = e.take_hook_with(&idle, false, || DIALOG).unwrap();
            assert_eq!((f.old, f.new, f.advance), (ClaudeStatus::Permission, ClaudeStatus::Permission, false));
            assert!(e.track.turn.open && e.track.turn.prompted, "die Runde gilt weiter");
            assert_eq!(e.track.perm_tool, "Bash");
        }
        // Ohne sichtbare Rückfrage gilt die Meldung – die Warteschlange läuft aber auch dann nicht los
        let f = e.take_hook_with(&HookMsg { kind: Some("idle_prompt".into()), ..msg("Notification") }, false, || NO_SCREEN).unwrap();
        assert_eq!((f.new, f.advance), (ClaudeStatus::Idle, false));
    }

    /// Review #282 (HIGH-1): Den Prompt der Shell kann jedes Programm vortäuschen. Ohne Bestätigung (im Vordergrund der
    /// Shell läuft noch etwas) ändert er nichts: die Freigabe-Frage bleibt, Meldungen zählen weiter, ein `claude -p`
    /// übernimmt nichts.
    #[test]
    fn a_spoofed_shell_prompt_changes_nothing() {
        let mut e = claude_entry();
        feed(&mut e, "SessionStart");
        feed(&mut e, "UserPromptSubmit");
        let ask = HookMsg { kind: Some("permission_prompt".into()), message: Some("Claude needs your permission to use Bash".into()), ..msg("Notification") };
        e.take_hook(&ask);
        assert!(e.note_shell_seq(2) && !e.note_shell_seq(2), "jedes Signal einmal");
        e.track.prompt_check = Some((Instant::now(), 0));
        // Claude läuft (hängt an der Konsole) bzw. es ist nicht zu ermitteln: kein Ende, bei jeder Abfrage
        for (i, idle) in [Some(false), None, Some(false), Some(false), None].into_iter().enumerate() {
            assert_eq!(e.prompt_checked(idle), None);
            assert_eq!(e.track.prompt_check.is_some(), i + 1 < PROMPT_TRIES_MS.len(), "nach der letzten Abfrage ist Schluss");
        }
        assert_eq!(e.prompt_checked(Some(true)), None, "ohne offenes Signal keine Abfrage");
        assert_eq!(e.status, ClaudeStatus::Permission, "die Freigabe-Frage schützt weiter vor Eingaben");
        assert!(e.track.gone.is_none() && !e.hook_conv.open);
        // Meldungen der zugeordneten Konversation zählen weiter, ein `claude -p` bleibt fremd (#260)
        let nested = HookMsg { claude_session: Some("N".into()), source: Some("startup".into()), ..msg("SessionStart") };
        assert!(e.take_hook(&nested).is_none());
        assert_eq!(e.take_hook_with(&HookMsg { tool_name: Some("Bash".into()), ..msg("PostToolUse") }, false, || NO_SCREEN).map(|f| f.new), Some(ClaudeStatus::Working));
        assert_eq!(e.hook_conv.bound.as_deref(), Some("A"));
    }

    /// Wo sich der Prompt nicht bestätigen lässt (WSL): Er nimmt nur „bereit“/„arbeitet“ weg – nie eine Rückfrage, nie
    /// eine Meldung, nie die Zuordnung.
    #[test]
    fn an_unconfirmable_prompt_only_clears_ready_and_working() {
        let mut e = claude_entry();
        e.track.confirmable = false;
        assert!(!e.prompt_unconfirmed(), "vor der ersten Meldung nichts");
        feed(&mut e, "SessionStart");
        feed(&mut e, "UserPromptSubmit");
        let ask = HookMsg { kind: Some("permission_prompt".into()), message: Some("x".into()), ..msg("Notification") };
        e.take_hook(&ask);
        assert!(!e.prompt_unconfirmed());
        assert_eq!(e.status, ClaudeStatus::Permission);
        assert!(!e.hook_conv.open && e.track.claude_seen);
        // Keine Meldung geht verloren (kein Nachzügler-Fenster), die Runde gilt weiter
        assert_eq!(feed(&mut e, "Stop"), ClaudeStatus::Idle);
        assert!(e.prompt_unconfirmed());
        assert_eq!(e.status, ClaudeStatus::Unknown);
        assert!(!e.track.turn.stale, "kein späteres „bereit“ aus dem Transkript: Claude ist vielleicht beendet");
        assert_eq!(e.track.launched, Launched::Running);
        let nested = HookMsg { claude_session: Some("N".into()), source: Some("startup".into()), ..msg("SessionStart") };
        assert!(e.take_hook(&nested).is_none(), "ein claude -p übernimmt nichts");
        assert_eq!(feed(&mut e, "UserPromptSubmit"), ClaudeStatus::Working, "Claude lebt doch: die nächste Meldung zählt");
    }

    /// Claude wird abgeschossen (kein SessionEnd) und im selben Terminal neu gestartet: vorher blieben alle Meldungen des
    /// neuen Claude „fremd“ und der Status für immer stehen. Jetzt – nach bestätigtem Ende – wird er zugeordnet.
    #[test]
    fn a_confirmed_end_clears_the_status_and_lets_a_new_claude_bind() {
        let mut e = claude_entry();
        feed(&mut e, "SessionStart");
        assert_eq!(feed(&mut e, "UserPromptSubmit"), ClaudeStatus::Working);
        e.meta.queue = vec!["rm -rf nicht in die Shell".into()];
        // Ein `claude -p` aus der Sitzung heraus bleibt fremd, solange Claude läuft (#260)
        let nested = HookMsg { claude_session: Some("N".into()), source: Some("startup".into()), ..msg("SessionStart") };
        assert!(e.take_hook(&nested).is_none());
        // Prompt der Shell, und in ihrem Vordergrund läuft nichts mehr: Claude ist weg
        e.note_shell_seq(2);
        e.track.prompt_check = Some((Instant::now(), 0));
        assert_eq!(e.prompt_checked(Some(true)), Some(true), "Warteschlange angehalten");
        assert_eq!(e.status, ClaudeStatus::Unknown);
        assert!(e.meta.queue_paused && e.track.prompt_check.is_none() && e.hook_conv.open);
        // Nachzügler des alten Claude zählen nicht
        assert!(e.take_hook(&msg("Stop")).is_none());
        assert_eq!(e.status, ClaudeStatus::Unknown);
        // `claude` von Hand neu gestartet: neue Konversation, wird zugeordnet – die Warteschlange bleibt stehen
        let fresh = |ev: &str| HookMsg { claude_session: Some("B".into()), source: Some("startup".into()), ..msg(ev) };
        let f = e.take_hook(&fresh("SessionStart")).expect("neuer Claude wird angenommen");
        assert_eq!((f.new, f.conv, f.advance), (ClaudeStatus::Idle, ConvMatch::Switch("B".into()), false));
        assert_eq!(e.take_hook(&fresh("UserPromptSubmit")).unwrap().new, ClaudeStatus::Working);
        assert!(!e.take_hook(&fresh("Stop")).unwrap().advance);
        // … und ab jetzt ist ein `claude -p` wieder fremd
        assert!(e.take_hook(&nested).is_none());
        assert_eq!(e.hook_conv.bound.as_deref(), Some("B"));
        // Eine offene Freigabe-Frage fällt mit dem bestätigten Ende weg (es gibt sie nicht mehr)
        let mut e = claude_entry();
        feed(&mut e, "SessionStart");
        e.take_hook(&HookMsg { kind: Some("permission_prompt".into()), ..msg("Notification") });
        e.track.prompt_check = Some((Instant::now(), 2));
        assert_eq!(e.prompt_checked(Some(true)), Some(false));
        assert_eq!(e.status, ClaudeStatus::Unknown);
    }

    /// Claude startete gar nicht (falsche Option, nicht installiert): Erst wenn die Shell eine Weile frei ist, darf ein
    /// von Hand gestarteter Claude sich zuordnen – nicht schon beim ersten Prompt, vor dem der Startbefehl noch läuft.
    #[test]
    fn a_failed_launch_is_noticed_only_after_the_shell_stayed_idle() {
        let mut e = claude_entry();
        e.meta.queue = vec!["Start-Prompt".into()];
        let fresh = HookMsg { claude_session: Some("B".into()), source: Some("startup".into()), ..msg("SessionStart") };
        assert!(e.note_shell_seq(2) && e.prompt_matters(), "zweiter Prompt: der Startbefehl (eine Zeile) ist durch");
        e.track.prompt_check = Some((Instant::now(), 0));
        assert_eq!(e.prompt_checked(Some(true)), None, "gleich nach dem Prompt: der Startbefehl wird erst gelesen");
        assert!(!e.hook_conv.open && e.track.launched == Launched::Expected);
        assert!(e.take_hook(&fresh).is_none(), "fremde Konversation bleibt fremd (#260)");
        e.track.prompt_check = Some((Instant::now() - NEVER_STARTED - Duration::from_millis(100), 4));
        assert_eq!(e.prompt_checked(Some(true)), Some(true), "Start-Prompt bleibt liegen, Warteschlange angehalten");
        assert!(e.hook_conv.open && e.track.launched == Launched::Ended);
        let f = e.take_hook(&fresh).expect("von Hand gestarteter Claude");
        assert_eq!((f.conv, f.advance), (ConvMatch::Bind("B".into()), false));
        // Läuft der Startbefehl (die Shell ist nicht frei), passiert nichts
        let mut e = claude_entry();
        e.note_shell_seq(2);
        e.track.prompt_check = Some((Instant::now() - NEVER_STARTED - Duration::from_millis(100), 4));
        assert_eq!(e.prompt_checked(Some(false)), None);
        assert!(!e.hook_conv.open && e.track.prompt_check.is_none());
        // Der Startbefehl ist laut Prompt-Zähler noch nicht durch (ein Startbefehl des Profils läuft in der Shell
        // selbst, an der Konsole hängt nur sie): kein Scheitern, auch wenn die Shell lange „frei“ aussieht
        let mut e = claude_entry();
        e.track.launch_lines = 2;
        e.note_shell_seq(1);
        assert!(!e.prompt_matters(), "nichts zu entscheiden: keine Abfrage der Konsole");
        e.track.prompt_check = Some((Instant::now() - NEVER_STARTED - Duration::from_millis(100), 4));
        assert_eq!(e.prompt_checked(Some(true)), None);
        assert!(!e.hook_conv.open && e.track.launched == Launched::Expected);
        // Irrtum (z. B. ein Theme mit doppeltem Prompt und ein langsamer Startbefehl): Der gestartete Claude meldet sich
        // doch mit seiner Konversation – er gilt wieder als der gestartete, die Warteschlange läuft
        let mut e = claude_entry();
        e.meta.queue = vec!["Start-Prompt".into()];
        e.note_shell_seq(2);
        e.track.prompt_check = Some((Instant::now() - NEVER_STARTED - Duration::from_millis(100), 4));
        assert_eq!(e.prompt_checked(Some(true)), Some(true));
        assert!(e.meta.queue_paused);
        let f = e.take_hook(&HookMsg { source: Some("startup".into()), ..msg("SessionStart") }).unwrap();
        assert_eq!((f.conv, f.advance), (ConvMatch::Bind("A".into()), true));
        assert!(!e.meta.queue_paused && e.track.launched == Launched::Running);
    }

    #[test]
    fn launch_lines_count_the_commands_typed_at_start() {
        assert_eq!(launch_lines(None), 0);
        assert_eq!(launch_lines(Some("claude --session-id x\r")), 1);
        assert_eq!(launch_lines(Some("cd app\rclaude\r")), 2);
    }

    /// Doppelt zugestellte Hooks: Je Runde zählt ein Stop (Review #282: nicht nach der Uhr).
    #[test]
    fn a_duplicated_stop_counts_once_per_turn() {
        let mut e = claude_entry();
        feed(&mut e, "SessionStart");
        feed(&mut e, "UserPromptSubmit");
        assert_eq!(e.take_hook(&msg("Stop")).map(|f| (f.old, f.new, f.advance)), Some((ClaudeStatus::Working, ClaudeStatus::Idle, true)));
        // Die Warteschlange hat inzwischen den nächsten Prompt geschickt
        e.assume_working();
        assert!(e.take_hook(&msg("Stop")).is_none(), "derselbe Stop noch einmal – auch viel später");
        assert_eq!(e.status, ClaudeStatus::Working);
        // Der Stop der nächsten Runde zählt wieder (dazwischen kam eine Meldung)
        feed(&mut e, "UserPromptSubmit");
        assert_eq!(e.take_hook(&msg("Stop")).map(|f| f.advance), Some(true));
    }

    /// Ohne Hooks kommt der Status aus dem Transkript – nach einem Prompt der Shell aber erst wieder, wenn es weitergeht.
    #[test]
    fn the_transcript_status_rests_after_a_shell_prompt() {
        let mut e = claude_entry();
        let now = std::time::SystemTime::now();
        assert!(e.transcript_status_ok(Some(now)));
        e.status = ClaudeStatus::Idle;
        e.track.prompt_check = Some((Instant::now(), 0));
        assert_eq!(e.prompt_checked(Some(true)), Some(false), "Status ohne Hooks: auch der endet am bestätigten Prompt");
        assert_eq!(e.status, ClaudeStatus::Unknown);
        assert!(!e.transcript_status_ok(Some(now - Duration::from_secs(5))), "seit dem Ende nichts geschrieben");
        assert!(!e.transcript_status_ok(None));
        assert!(e.transcript_status_ok(Some(now + Duration::from_secs(60))), "es wird wieder geschrieben");
        assert!(e.track.gone.is_none());
        // WSL: ohne Bestätigung genauso
        let mut e = claude_entry();
        e.status = ClaudeStatus::Working;
        assert!(e.prompt_unconfirmed());
        assert!(!e.transcript_status_ok(Some(now - Duration::from_secs(5))));
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

    /// #302: Nur die Konversation der Sitzung wird gespeichert (und später mit `--resume` fortgesetzt). Ein Claude, der
    /// nach dem Ende des gestarteten im selben Terminal läuft (von Hand, ein übrig gebliebener Prozess mit dem Schlüssel
    /// der Sitzung), ändert sie nie – auch nicht „Inhalt vorhanden“.
    #[test]
    fn only_the_sessions_own_conversation_is_kept_for_resume() {
        let with = |ev: &str, id: &str, src: Option<&str>, reason: Option<&str>| HookMsg {
            claude_session: Some(id.into()),
            source: src.map(str::to_string),
            reason: reason.map(str::to_string),
            ..msg(ev)
        };
        let deliver = |e: &mut Entry, m: HookMsg| {
            if let Some(f) = e.take_hook(&m) {
                e.note_conversation(&m, &f.conv, f.keyless);
            }
        };
        let fresh = || {
            let mut e = claude_entry();
            e.meta.claude_session = "A".into();
            e
        };
        // Gestarteter Claude meldet sich, Prompt: eigene Konversation mit Inhalt
        let mut e = fresh();
        deliver(&mut e, with("SessionStart", "A", Some("startup"), None));
        deliver(&mut e, with("UserPromptSubmit", "A", None, None));
        assert_eq!((e.meta.claude_session.as_str(), e.meta.claude_started), ("A", true));
        // /clear im gestarteten Claude: neue eigene Konversation
        deliver(&mut e, with("SessionEnd", "A", None, Some("clear")));
        deliver(&mut e, with("SessionStart", "C", Some("clear"), None));
        assert_eq!((e.meta.claude_session.as_str(), e.meta.claude_started), ("C", false));
        deliver(&mut e, with("UserPromptSubmit", "C", None, None));
        assert!(e.meta.claude_started);
        // Claude beendet (/exit), danach `claude` von Hand im selben Terminal: bleibt C
        deliver(&mut e, with("SessionEnd", "C", None, Some("prompt_input_exit")));
        deliver(&mut e, with("SessionStart", "B", Some("startup"), None));
        deliver(&mut e, with("UserPromptSubmit", "B", None, None));
        assert_eq!(e.meta.claude_session, "C", "von Hand gestarteter Claude wird nicht zur Konversation der Sitzung");
        assert_eq!(e.hook_conv.bound.as_deref(), Some("B"), "sein Status wird trotzdem verfolgt");
        // Auch sein /resume nicht
        deliver(&mut e, with("SessionStart", "D", Some("resume"), None));
        assert_eq!(e.meta.claude_session, "C");

        // Gestarteter Claude kam nie (Prompt der Shell), ein anderer meldet sich: die Sitzung behält ihre nie benutzte ID
        // ohne Inhalt – Neu laden beginnt neu statt `--resume` mit fremder ID
        let mut e = fresh();
        e.hook_conv.shell_prompt();
        deliver(&mut e, with("SessionStart", "B", Some("startup"), None));
        deliver(&mut e, with("UserPromptSubmit", "B", None, None));
        assert_eq!((e.meta.claude_session.as_str(), e.meta.claude_started), ("A", false));
        // Meldet sich doch noch der gestartete Claude (seine ID), gehört sie der Sitzung
        let mut e = fresh();
        e.hook_conv.shell_prompt();
        deliver(&mut e, with("UserPromptSubmit", "A", None, None));
        assert_eq!((e.meta.claude_session.as_str(), e.meta.claude_started), ("A", true));
        // Vorgetäuschter Prompt der Shell: die eigene Konversation meldet sich weiter und bleibt die der Sitzung
        let mut e = fresh();
        deliver(&mut e, with("SessionStart", "A", Some("startup"), None));
        e.hook_conv.shell_prompt();
        deliver(&mut e, with("PreToolUse", "A", None, None));
        deliver(&mut e, with("SessionEnd", "A", None, Some("clear")));
        deliver(&mut e, with("SessionStart", "C", Some("clear"), None));
        assert_eq!(e.meta.claude_session, "C");

        // `--continue` beim Anlegen (keine ID übergeben): die fortgesetzte Konversation gehört der Sitzung
        let mut e = claude_entry();
        e.hook_conv = hooks::ConvBinding::launch(None, false);
        deliver(&mut e, with("SessionStart", "Z", Some("resume"), None));
        assert_eq!((e.meta.claude_session.as_str(), e.meta.claude_started), ("Z", true));

        // Review: vorgetäuschter Prompt der Shell, gleich danach /clear (ohne Meldung dazwischen): C gehört der Sitzung
        let mut e = fresh();
        deliver(&mut e, with("SessionStart", "A", Some("startup"), None));
        e.hook_conv.shell_prompt();
        deliver(&mut e, with("SessionEnd", "A", None, Some("clear")));
        deliver(&mut e, with("SessionStart", "C", Some("clear"), None));
        assert_eq!(e.meta.claude_session, "C");

        // Review: /resume im gestarteten Claude mit SessionEnd-Grund `other` (oder ohne Grund): die fortgesetzte gehört der
        // Sitzung; ein danach neu gestarteter Claude (`startup`) nie
        for reason in [Some("other"), None, Some("resume")] {
            let mut e = fresh();
            deliver(&mut e, with("SessionStart", "A", Some("startup"), None));
            deliver(&mut e, with("SessionEnd", "A", None, reason));
            deliver(&mut e, with("SessionStart", "D", Some("resume"), None));
            assert_eq!((e.meta.claude_session.as_str(), e.meta.claude_started), ("D", true), "{reason:?}");
            let mut e = fresh();
            deliver(&mut e, with("SessionStart", "A", Some("startup"), None));
            deliver(&mut e, with("SessionEnd", "A", None, reason));
            deliver(&mut e, with("SessionStart", "B", Some("startup"), None));
            deliver(&mut e, with("UserPromptSubmit", "B", None, None));
            assert_eq!((e.meta.claude_session.as_str(), e.meta.claude_started), ("A", false), "{reason:?}");
            // … und auch dessen spätere Wechsel nicht
            deliver(&mut e, with("SessionStart", "E", Some("clear"), None));
            assert_eq!(e.meta.claude_session, "A", "{reason:?}");
        }
        // Eindeutig beendet: auch ein `claude --resume X` von Hand gehört nicht der Sitzung
        for reason in ["prompt_input_exit", "logout", "bypass_permissions_disabled"] {
            let mut e = fresh();
            deliver(&mut e, with("SessionStart", "A", Some("startup"), None));
            deliver(&mut e, with("SessionEnd", "A", None, Some(reason)));
            deliver(&mut e, with("SessionStart", "X", Some("resume"), None));
            assert_eq!((e.meta.claude_session.as_str(), e.meta.claude_started), ("A", false), "{reason}");
        }

        // Review: Reihenfolge vertauscht – SessionStart(clear) der neuen vor SessionEnd(clear) der alten Konversation
        let mut e = fresh();
        deliver(&mut e, with("SessionStart", "A", Some("startup"), None));
        deliver(&mut e, with("SessionStart", "C", Some("clear"), None));
        deliver(&mut e, with("SessionEnd", "A", None, Some("clear")));
        assert_eq!((e.meta.claude_session.as_str(), e.hook_conv.bound.as_deref(), e.hook_conv.own), ("C", Some("C"), true));
        deliver(&mut e, with("UserPromptSubmit", "C", None, None));
        assert!(e.meta.claude_started);

        // Review: Transkript (Kosten, MCP) nur von der eigenen Konversation
        let tp = |ev: &str, id: &str, src: Option<&str>, path: &str| HookMsg { transcript_path: Some(path.into()), ..with(ev, id, src, None) };
        let mut e = fresh();
        let m = tp("SessionStart", "A", Some("startup"), "C:\\p\\A.jsonl");
        let f = e.take_hook(&m).unwrap();
        assert!(e.note_conversation(&m, &f.conv, f.keyless), "eigenes Transkript übernommen");
        assert!(!e.note_conversation(&m, &f.conv, f.keyless), "unverändert");
        deliver(&mut e, with("SessionEnd", "A", None, Some("prompt_input_exit")));
        let m = tp("SessionStart", "B", Some("startup"), "C:\\p\\B.jsonl");
        let f = e.take_hook(&m).unwrap();
        assert!(!e.note_conversation(&m, &f.conv, f.keyless));
        let m = tp("Stop", "B", None, "C:\\p\\B.jsonl");
        let f = e.take_hook(&m).unwrap();
        assert!(!e.note_conversation(&m, &f.conv, f.keyless));
        assert_eq!(e.meta.transcript, "C:\\p\\A.jsonl");

        // Review: Meldungen ohne Schlüssel (globale Hooks einer älteren paneldeck.exe) ändern nie die Konversation
        let mut e = fresh();
        let keyless = |ev: &str, id: &str, src: Option<&str>| HookMsg { key: None, transcript_path: Some("C:\\p\\K.jsonl".into()), ..with(ev, id, src, None) };
        for m in [keyless("SessionStart", "A", Some("resume")), keyless("UserPromptSubmit", "A", None)] {
            let f = e.take_hook(&m).expect("für den Status angenommen");
            assert!(f.keyless);
            assert!(!e.note_conversation(&m, &f.conv, f.keyless));
        }
        assert_eq!((e.meta.claude_session.as_str(), e.meta.claude_started, e.meta.transcript.as_str()), ("A", false, ""));
        // Auch wenn die Zuordnung (aus Meldungen mit Schlüssel) der Sitzung gehört
        deliver(&mut e, with("SessionStart", "A", Some("startup"), None));
        assert!(e.hook_conv.own);
        assert!(!e.note_conversation(&keyless("UserPromptSubmit", "A", None), &ConvMatch::Same, true));
        assert!(!e.meta.claude_started);
    }
}
