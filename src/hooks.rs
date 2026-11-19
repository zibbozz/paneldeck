//! Claude-Code-Hooks: Status der Claude-Sitzungen ohne Bildschirmanalyse.
//!
//! Ablauf: Claude ruft bei Ereignissen `paneldeck.exe --hook` auf (Hook-Befehl). Dieser Aufruf liest
//! das Ereignis-JSON von stdin und schickt es per TCP an die laufende Hub-Instanz (127.0.0.1).
//! Sitzung und Token kommen über Umgebungsvariablen, die der Hub in jedes Terminal setzt.

use eframe::egui;
use serde_json::{json, Value};
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

pub const ENV_SESSION: &str = "PANELDECK_SESSION";
/// Geheimer Schlüssel der Sitzung (#152, 128 Bit, je Start neu, nie in Oberfläche oder Protokoll). Nur wer ihn kennt,
/// kann sich als diese Sitzung ausweisen, wenn der Prozessbaum nicht hilft (WSL); `PANELDECK_SESSION` allein reicht nicht.
pub const ENV_SESSION_KEY: &str = "PANELDECK_SESSION_KEY";
pub const ENV_PORT: &str = "PANELDECK_PORT";
pub const ENV_TOKEN: &str = "PANELDECK_TOKEN";

/// Ereignisse, für die Paneldeck sich einträgt. Nur solche, die Claude Code schon so lange kennt wie `SessionEnd`
/// (1.0.85): Ein unbekannter Ereignisname macht für ältere Versionen die ganze Settings-Datei ungültig – neuere
/// Ereignisse (`StopFailure`, `PermissionRequest`, `PostToolUseFailure` …) fehlen deshalb bewusst; was sie melden
/// würden, fängt der Rückfall in [`stale_status`] ab.
const EVENTS: [&str; 9] = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "Notification",
    "Stop",
    "SubagentStop",
    "PreCompact",
    "SessionEnd",
];

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ClaudeStatus {
    /// Keine Hook-Daten (kein Claude oder Hooks nicht aktiv).
    #[default]
    Unknown,
    /// Claude läuft, wartet auf den nächsten Prompt.
    Idle,
    Working,
    /// Claude fragt nach einer Freigabe.
    Permission,
    /// Claude braucht sonstige Eingabe.
    Input,
}

#[derive(Debug, Clone, Default)]
pub struct HookMsg {
    pub session: u64,
    pub event: String,
    pub kind: Option<String>,
    pub message: Option<String>,
    pub claude_session: Option<String>,
    pub transcript_path: Option<String>,
    pub cwd: Option<String>,
    /// `PANELDECK_SESSION_KEY` des Absenders (#260): nur Meldungen mit dem Schlüssel des laufenden Starts zählen.
    pub key: Option<String>,
    /// `source` bei SessionStart (`startup`, `resume`, `clear`, `compact`).
    pub source: Option<String>,
    /// Werkzeug bei PreToolUse/PostToolUse.
    pub tool_name: Option<String>,
    /// Unter-Agent, in dem das Ereignis auftrat (fehlt beim Haupt-Agenten und bei älteren Claude-Versionen).
    pub agent_id: Option<String>,
    /// `trigger` bei PreCompact (`manual`, `auto`).
    pub trigger: Option<String>,
    /// `reason` bei SessionEnd (`clear`, `resume`, `logout`, `prompt_input_exit`, `other`).
    pub reason: Option<String>,
}

/// Meldet SessionEnd, dass Claude wirklich beendet ist? Bei `/clear` und `/resume` endet nur die Konversation –
/// derselbe Claude läuft mit einer anderen weiter.
pub fn session_end_is_exit(m: &HookMsg) -> bool {
    m.event == "SessionEnd" && !matches!(m.reason.as_deref(), Some("clear" | "resume"))
}

/// Stammt die Meldung vom laufenden Start der Sitzung (#260)? `current` = Schlüssel dieses Starts (`Entry::session_key`).
/// Ohne oder mit falschem Schlüssel (anderes Terminal, Prozess eines früheren Starts, Fälschung) nie.
pub fn key_matches(current: &str, sent: Option<&str>) -> bool {
    let Some(sent) = sent else { return false };
    // Vergleich ohne frühen Abbruch (der Schlüssel ist ein Geheimnis).
    !current.is_empty() && current.len() == sent.len() && current.bytes().zip(sent.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

/// Annahme einer Hook-Meldung für eine Sitzung (#260).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admit {
    /// Verwerfen.
    Drop,
    /// Mit dem Schlüssel des laufenden Starts: voll verarbeiten.
    Keyed,
    /// Ganz ohne Schlüssel (globale Hooks einer älteren paneldeck.exe): nur für den Status, nie für Konversation,
    /// Transkript oder „Prompt gesendet“.
    StatusOnly,
}

/// `current` = Schlüssel des laufenden Starts, `keyed_seen` = in diesem Start kam schon eine Meldung mit Schlüssel
/// (dann stammen solche ohne nicht vom Claude dieser Sitzung), `conv` = Zuordnung der Konversation.
pub fn admit(current: &str, keyed_seen: bool, conv: &ConvBinding, m: &HookMsg) -> Admit {
    match m.key.as_deref() {
        Some(k) if key_matches(current, Some(k)) => Admit::Keyed,
        Some(_) => Admit::Drop,
        None if current.is_empty() || keyed_seen || conv.is_foreign(m) => Admit::Drop,
        None => Admit::StatusOnly,
    }
}

/// Zuordnung einer Meldung zur Konversation des laufenden Starts (#260, #245).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConvMatch {
    /// Meldung ohne Konversations-ID: normal verarbeiten.
    NoId,
    /// Erste Konversation dieses Starts: übernehmen.
    Bind(String),
    /// Dieselbe Konversation.
    Same,
    /// Claude wechselt die Konversation (`/clear`, `/resume`, Zusammenfassen): übernehmen.
    Switch(String),
    /// Eine andere Konversation (z. B. ein von Claude gestartetes `claude -p` mit geerbter Umgebung): ignorieren.
    Foreign,
}

/// Konversation, die zum laufenden Start einer Sitzung gehört (#260, #245). Je Start neu ([`ConvBinding::launch`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConvBinding {
    /// Zugeordnete Konversation des laufenden Claude.
    pub bound: Option<String>,
    /// Beim Start übergebene Konversations-ID (`--session-id`/`--resume`): solange noch nichts zugeordnet ist, zählt
    /// nur sie. Fällt weg, sobald der beim Start aufgerufene Claude endet.
    pub expect: Option<String>,
    /// Der Start hat die Hooks abgeschaltet (`--safe-mode`): alle Meldungen stammen dann von anderen Prozessen (z. B.
    /// einem `claude -p` mit den globalen Hooks) und zählen nie.
    pub hooks_off: bool,
    /// Die Shell steht nach dem Start wieder an ihrem Prompt ([`ConvBinding::shell_prompt`]): der Claude dieser
    /// Zuordnung ist beendet, ohne es zu melden (abgeschossen, abgestürzt, SessionEnd verloren). Ein SessionStart einer
    /// neuen Konversation darf dann übernehmen – ein im selben Terminal neu gestarteter Claude bliebe sonst für immer
    /// „fremd“ und der Status eingefroren. Fällt weg, sobald sich die zugeordnete Konversation wieder meldet (sie lebt
    /// also doch) oder eine neue zugeordnet ist.
    pub open: bool,
}

impl ConvBinding {
    pub fn launch(expect: Option<String>, hooks_off: bool) -> Self {
        ConvBinding { bound: None, expect: expect.filter(|e| !e.is_empty()), hooks_off, open: false }
    }

    /// Die Shell zeigt nach dem Startbefehl wieder ihren Prompt: Was darin an Claude lief, ist beendet. Ein von Claude
    /// gestartetes `claude -p` kann das nicht auslösen – solange Claude im Vordergrund läuft, gibt die Shell keinen
    /// Prompt aus.
    pub fn shell_prompt(&mut self) {
        self.open = true;
    }

    /// Meldung `m` zuordnen und den Zustand fortschreiben. Eine neue Konversation zählt nur
    /// - als erste des Starts: mit der beim Start übergebenen ID, ohne solche (`--continue`) nur über SessionStart;
    /// - nach dem Ende der zugeordneten Konversation (SessionEnd, z. B. Claude beendet und im selben Terminal neu
    ///   gestartet): wieder über SessionStart;
    /// - als Wechsel über SessionStart mit `source` `clear`/`resume`/`compact`;
    /// - über SessionStart, nachdem die Shell wieder an ihrem Prompt stand ([`ConvBinding::shell_prompt`]): der bisherige
    ///   Claude ist dann beendet, auch wenn sein SessionEnd nie ankam.
    ///
    /// Alles andere mit fremder ID (z. B. ein von Claude gestartetes `claude -p` mit geerbter Umgebung) wird verworfen.
    pub fn feed(&mut self, m: &HookMsg) -> ConvMatch {
        if self.hooks_off {
            return ConvMatch::Foreign;
        }
        let Some(id) = m.claude_session.as_deref().filter(|c| !c.is_empty()) else { return ConvMatch::NoId };
        let start = m.event == "SessionStart";
        let r = match self.bound.as_deref() {
            Some(b) if b == id => ConvMatch::Same,
            Some(_) if start && (self.open || matches!(m.source.as_deref(), Some("clear" | "resume" | "compact"))) => ConvMatch::Switch(id.to_string()),
            Some(_) => ConvMatch::Foreign,
            None => match self.expect.as_deref() {
                Some(x) if x == id => ConvMatch::Bind(id.to_string()),
                Some(_) if start && self.open => ConvMatch::Bind(id.to_string()),
                Some(_) => ConvMatch::Foreign,
                None if start => ConvMatch::Bind(id.to_string()),
                None => ConvMatch::Foreign,
            },
        };
        match &r {
            ConvMatch::Bind(id) | ConvMatch::Switch(id) => {
                self.bound = Some(id.clone());
                self.expect = None;
                self.open = false;
            }
            // Der Claude dieser Konversation ist beendet: ein im selben Terminal neu gestarteter Claude darf sich mit
            // seinem SessionStart wieder zuordnen.
            ConvMatch::Same if m.event == "SessionEnd" => {
                self.bound = None;
                self.expect = None;
                self.open = false;
            }
            // Die zugeordnete Konversation meldet sich weiter: sie lebt, der Prompt der Shell hat getäuscht (Claude
            // angehalten, ungewöhnlicher Startbefehl). Ab jetzt wieder streng – ein `claude -p` übernimmt nichts.
            ConvMatch::Same => self.open = false,
            _ => {}
        }
        r
    }

    /// Ohne Zustandsänderung: gehört die Meldung sicher zu einer anderen Konversation (für Meldungen ohne Schlüssel,
    /// die nie etwas zuordnen dürfen)?
    pub fn is_foreign(&self, m: &HookMsg) -> bool {
        if self.hooks_off {
            return true;
        }
        // Nach dem Ende des bisherigen Claude ([`ConvBinding::shell_prompt`]) ist eine andere ID nicht mehr sicher fremd.
        if self.open {
            return false;
        }
        let Some(id) = m.claude_session.as_deref().filter(|c| !c.is_empty()) else { return false };
        self.bound.as_deref().or(self.expect.as_deref()).is_some_and(|b| b != id)
    }
}

/// Was der Hub über die laufende Runde des Haupt-Agenten weiß (je Start neu).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnState {
    /// Der Haupt-Agent steckt in einer Runde: ein Prompt wurde abgeschickt oder er selbst hat ein Werkzeug benutzt, und
    /// seither kam kein Stop. Unterscheidet „Unter-Agent im Hintergrund fertig“ (dann bereit) von „Unter-Agent in einer
    /// laufenden Runde fertig“ und sagt, ob Claude nach einem automatischen Zusammenfassen weiterarbeitet.
    pub open: bool,
    /// Die Runde begann mit einem abgeschickten Prompt (UserPromptSubmit). Nur das Ende einer solchen Runde lässt die
    /// Warteschlange weiterlaufen – nicht ein SessionStart, eine Benachrichtigung oder das Ende eines Hintergrund-Agenten.
    pub prompted: bool,
    /// „arbeitet“ ist nur angenommen (der Hub hat einen Prompt geschickt, das Zusammenfassen hat begonnen) und noch von
    /// keiner Meldung bestätigt: Bleibt die Bestätigung aus, gilt wieder dieser Status ([`stale_status`]).
    pub tentative: Option<ClaudeStatus>,
    /// Der Status ist unbekannt, weil Meldungen ausblieben ([`stale_status`]) – nicht, weil Claude beendet ist. Belegt
    /// das Transkript danach doch noch das Ende der Runde, gilt „bereit“.
    pub stale: bool,
}

/// Status einer Benachrichtigung. Ältere Claude-Versionen senden kein `notification_type`; dann entscheidet der Text.
fn notification_status(cur: ClaudeStatus, m: &HookMsg) -> ClaudeStatus {
    match m.kind.as_deref() {
        Some("permission_prompt") => ClaudeStatus::Permission,
        Some("idle_prompt") => ClaudeStatus::Idle,
        // Reine Auskünfte: Claude braucht dafür niemanden.
        Some("auth_success" | "elicitation_complete" | "elicitation_response") => cur,
        Some(_) => ClaudeStatus::Input,
        None => {
            let text = m.message.as_deref().unwrap_or("").to_lowercase();
            if text.contains("permission") || text.contains("approval") {
                ClaudeStatus::Permission
            } else if text.contains("waiting for your input") {
                ClaudeStatus::Idle
            } else {
                ClaudeStatus::Input
            }
        }
    }
}

/// Stellt die Meldung eine Rückfrage (Freigabe oder sonstige Eingabe)? Auch dann, wenn schon eine offen war: Claude
/// zeigt dann die nächste.
pub fn notification_asks(m: &HookMsg) -> bool {
    m.event == "Notification" && matches!(notification_status(ClaudeStatus::Unknown, m), ClaudeStatus::Permission | ClaudeStatus::Input)
}

/// Werkzeug, für das Claude laut Benachrichtigung eine Freigabe braucht („Claude needs your permission to use Bash“).
pub fn permission_tool(message: &str) -> Option<&str> {
    let rest = message.trim().strip_prefix("Claude needs your permission to use ")?;
    let tool = rest.trim().trim_end_matches('.');
    (!tool.is_empty()).then_some(tool)
}

/// Status nach einer Hook-Meldung; `turn` wird fortgeschrieben.
///
/// Nicht jede Meldung heißt, was ihr Name nahelegt:
/// - `SessionStart` mit `source` `compact` kommt auch mitten in einer Runde (automatisches Zusammenfassen) – Claude
///   arbeitet danach weiter; „bereit“ würde der Warteschlange den nächsten Prompt mitten in die Arbeit schicken.
/// - Werkzeug-Meldungen eines Unter-Agenten (`agent_id`) kommen auch, nachdem der Haupt-Agent fertig ist
///   (Hintergrund-Agenten); ihr Ende meldet `SubagentStop`, nicht `Stop`.
/// - `Stop` fehlt nach einer Unterbrechung (Esc), einem API-Fehler oder einer abgelehnten Freigabe ganz – das fängt
///   [`stale_status`] ab.
pub fn status_after(cur: ClaudeStatus, turn: &mut TurnState, m: &HookMsg) -> ClaudeStatus {
    // Jede Meldung bestätigt oder ersetzt eine Annahme.
    let assumed = turn.tentative.take();
    turn.stale = false;
    match m.event.as_str() {
        "SessionStart" if m.source.as_deref() == Some("compact") => {
            if turn.open {
                ClaudeStatus::Working
            } else {
                ClaudeStatus::Idle
            }
        }
        "SessionStart" | "Stop" => {
            turn.open = false;
            turn.prompted = false;
            ClaudeStatus::Idle
        }
        "UserPromptSubmit" => {
            turn.open = true;
            turn.prompted = true;
            ClaudeStatus::Working
        }
        "PreToolUse" | "PostToolUse" => {
            if m.agent_id.as_deref().is_none_or(str::is_empty) {
                turn.open = true;
            }
            ClaudeStatus::Working
        }
        // Ein Unter-Agent ist fertig. In einer Runde arbeitet der Haupt-Agent weiter; außerhalb (Hintergrund-Agent) ist
        // Claude damit wieder bereit.
        "SubagentStop" if cur == ClaudeStatus::Working && !turn.open => ClaudeStatus::Idle,
        "PreCompact" => {
            // `/compact` gibt es nur, wenn der Haupt-Agent nicht arbeitet; von selbst fasst Claude nur in einer Runde
            // zusammen (auch in einer, die ohne Prompt begann) und arbeitet danach weiter.
            match m.trigger.as_deref() {
                Some("manual") => turn.open = false,
                Some("auto") => turn.open = true,
                _ => {}
            }
            match cur {
                // Wird das Zusammenfassen abgebrochen, kommt nichts mehr: dann gilt wieder der Status von vorher.
                ClaudeStatus::Idle | ClaudeStatus::Unknown => {
                    turn.tentative = Some(cur);
                    ClaudeStatus::Working
                }
                _ => {
                    turn.tentative = assumed;
                    cur
                }
            }
        }
        "Notification" => {
            let new = notification_status(cur, m);
            if new == ClaudeStatus::Idle {
                turn.open = false;
                turn.prompted = false;
            }
            if new == cur {
                turn.tentative = assumed;
            }
            new
        }
        "SessionEnd" => {
            *turn = TurnState::default();
            ClaudeStatus::Unknown
        }
        _ => {
            turn.tentative = assumed;
            cur
        }
    }
}

/// Was der Bildschirm einer Claude-Sitzung zeigt – nur als Gegenprobe für [`stale_status`], nie allein entscheidend.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScreenHints {
    /// Die Arbeitsanzeige („… esc to interrupt“) steht unten im Bild.
    pub spinner: bool,
    /// Es sieht nach einer offenen Rückfrage aus (Freigabe, Auswahl).
    pub dialog: bool,
}

/// Hinweise aus dem sichtbaren Bildschirm (`Session::screen_text`). Verglichen wird ohne Leerraum und ohne
/// Groß-/Kleinschreibung: In schmalen Bereichen bricht Claude die Zeile um.
pub fn screen_hints(screen: &str) -> ScreenHints {
    let lines: Vec<&str> = screen.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail = |n: usize| -> String { lines[lines.len().saturating_sub(n)..].iter().flat_map(|l| l.chars()).filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect() };
    // Beides steht am unteren Ende: die Arbeitsanzeige über dem Eingabefeld (dazwischen höchstens die Aufgabenliste),
    // Frage, Auswahl und Fußzeile einer Rückfrage ganz unten. Text weiter oben (die Unterhaltung) zählt nicht.
    let (work, ask) = (tail(20), tail(12));
    ScreenHints { spinner: work.contains("esctointerrupt"), dialog: ["esctocancel", "doyouwantto", "wouldyouliketoproceed", "tellclaudewhattodo"].iter().any(|p| ask.contains(p)) }
}

/// Was der Hub außer den Hook-Meldungen über eine Claude-Sitzung beobachtet (für [`stale_status`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Observed {
    /// Zeit seit der letzten Hook-Meldung bzw. seit der Hub den Status selbst gesetzt hat.
    pub since_event: Duration,
    /// So lange hat das Programm nichts mehr ausgegeben.
    pub quiet: Duration,
    pub screen: ScreenHints,
    /// Zeit seit der letzten Eingabe ins Terminal, wenn sie nach der letzten Meldung kam (Esc, Antwort auf eine Rückfrage).
    pub input_since: Option<Duration>,
    /// Laut Transkript ist die Runde nach der letzten Meldung zu Ende gegangen (unterbrochen, API-Fehler, Antwort fertig).
    pub turn_ended: bool,
}

/// Ruhe nach einem belegten Ende der Runde, bevor der Status wechselt.
pub const STALE_SETTLE: Duration = Duration::from_secs(2);
/// So lange darf ein nur angenommenes „arbeitet“ ([`TurnState::tentative`]) unbestätigt bleiben.
pub const STALE_TENTATIVE: Duration = Duration::from_secs(10);
/// So lange darf „arbeitet“ ohne Meldung, ohne Ausgabe und ohne Arbeitsanzeige stehen. Länger als Claude braucht, um eine
/// Freigabe-Frage zu melden (die Benachrichtigung kommt erst nach einigen Sekunden ohne Eingabe).
pub const STALE_WORKING: Duration = Duration::from_secs(20);
/// Kürzere Frist, wenn der Nutzer seit der letzten Meldung ins Terminal getippt hat (meist Esc) und es seither still ist.
pub const STALE_AFTER_INPUT: Duration = Duration::from_secs(5);

/// Rückfall, wenn Hook-Meldungen ausbleiben: neuer Status oder `None` (so lassen).
///
/// Claude meldet das Ende einer Runde nicht immer – kein `Stop` nach Esc, nach einem API-Fehler oder einer im Terminal
/// abgelehnten Freigabe, keine Meldung nach einer im Terminal erteilten Freigabe, bis das Werkzeug fertig ist. Ohne
/// Rückfall bliebe „arbeitet“ bzw. „Freigabe?“ stehen, bis der nächste Prompt durch ist.
///
/// - „arbeitet“ → „bereit“, wenn das Transkript das Ende der Runde belegt (`turn_ended`: ein Eintrag, der in der Datei
///   hinter dem Stand bei der letzten Meldung steht) und keine Rückfrage zu sehen ist;
/// - angenommenes „arbeitet“ → Status von vorher, wenn nichts es bestätigt;
/// - „arbeitet“ → unbekannt, wenn lange nichts kam (Meldung verloren, Claude abgeschossen) oder der Nutzer getippt hat
///   und es seither still ist (Esc, bevor etwas im Transkript stand): lieber kein Status als ein falscher – „bereit“
///   ohne Beleg nie, sonst tippte ein Agent oder die Warteschlange womöglich in eine offene Rückfrage. Kommt der Beleg
///   später doch, wird daraus „bereit“;
/// - „Freigabe?“/„Eingabe“ → „bereit“ mit Beleg aus dem Transkript; → „arbeitet“ nur, wenn der Nutzer ins Terminal
///   getippt hat, die Arbeitsanzeige läuft und keine Rückfrage zu sehen ist.
///
/// Solange die Arbeitsanzeige zu sehen ist, bleibt „arbeitet“ in jedem Fall.
pub fn stale_status(cur: ClaudeStatus, turn: &TurnState, o: &Observed) -> Option<ClaudeStatus> {
    let calm = |d: Duration| o.since_event >= d && o.quiet >= d;
    match cur {
        ClaudeStatus::Working => {
            if o.screen.spinner {
                None
            } else if o.turn_ended && !o.screen.dialog && calm(STALE_SETTLE) {
                Some(ClaudeStatus::Idle)
            } else if let Some(prev) = turn.tentative {
                calm(STALE_TENTATIVE).then_some(prev)
            } else if calm(STALE_WORKING) || (o.quiet >= STALE_AFTER_INPUT && o.input_since.is_some_and(|d| d >= STALE_AFTER_INPUT)) {
                Some(ClaudeStatus::Unknown)
            } else {
                None
            }
        }
        ClaudeStatus::Unknown if turn.stale && o.turn_ended && !o.screen.spinner && !o.screen.dialog && calm(STALE_SETTLE) => Some(ClaudeStatus::Idle),
        ClaudeStatus::Permission | ClaudeStatus::Input => {
            // Auch mit Beleg nie, solange eine Rückfrage zu sehen ist (z. B. die eines Hintergrund-Agenten gleich nach
            // dem Ende der Runde): „bereit“ ließe Agenten und die Warteschlange hineintippen.
            if o.turn_ended && !o.screen.dialog && calm(STALE_SETTLE) {
                Some(ClaudeStatus::Idle)
            } else if o.screen.spinner && !o.screen.dialog && o.quiet < Duration::from_secs(3) && o.input_since.is_some_and(|d| d >= Duration::from_secs(1)) {
                Some(ClaudeStatus::Working)
            } else {
                None
            }
        }
        ClaudeStatus::Idle | ClaudeStatus::Unknown => None,
    }
}

/// Darf eine Werkzeug-Meldung (PreToolUse/PostToolUse) eine offene Rückfrage („Freigabe?“/„Eingabe“) beenden?
///
/// Mehrere Werkzeuge oder Unter-Agenten laufen oft gleichzeitig: Die Meldung eines anderen Werkzeugs sagt nichts über
/// die Rückfrage – der Status sprang sonst auf „arbeitet“, während Claude weiter auf die Antwort wartet, und Claude
/// meldet die Frage kein zweites Mal. Beantwortet ist sie, wenn der Nutzer seither ins Terminal getippt hat, genau das
/// Werkzeug fertig ist, für das gefragt wurde, oder die Arbeitsanzeige wieder läuft (Antwort von anderswo, z. B. per
/// Fernsteuerung; solange eine Rückfrage offen ist, zeigt Claude sie nicht) – und keine Rückfrage mehr zu sehen ist.
pub fn tool_event_answers(m: &HookMsg, awaited_tool: &str, input_since: bool, screen: ScreenHints) -> bool {
    let same_tool = m.event == "PostToolUse" && !awaited_tool.is_empty() && m.tool_name.as_deref() == Some(awaited_tool);
    (input_since || same_tool || screen.spinner) && !screen.dialog
}

/// 32 Zufallsbytes aus dem Betriebssystem (Hex). Fällt nur im Notfall auf einen schwächeren Wert zurück.
pub(crate) fn random_hex() -> String {
    let mut b = [0u8; 32];
    if getrandom::fill(&mut b).is_err() {
        let a = RandomState::new().build_hasher().finish();
        let c = RandomState::new().build_hasher().finish();
        b[..8].copy_from_slice(&a.to_le_bytes());
        b[8..16].copy_from_slice(&c.to_le_bytes());
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// UUID v4 (für `claude --session-id`).
pub fn new_uuid() -> String {
    let h = random_hex();
    let mut b: Vec<u8> = (0..16).map(|i| u8::from_str_radix(&h[i * 2..i * 2 + 2], 16).unwrap_or(0)).collect();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let x: String = b.iter().map(|v| format!("{v:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &x[0..8], &x[8..12], &x[12..16], &x[16..20], &x[20..32])
}

/// Befehle einer zweiten Programminstanz (z. B. Explorer-Kontextmenü).
#[derive(Debug, Clone, PartialEq)]
pub enum IpcCmd {
    Show,
    /// `origin`: Prozessbaum des Absenders – ermittelt der Hub selbst aus der Verbindung (#152), nie aus der Anfrage.
    /// Liegt eine Sitzung darin, erbt die neue Sitzung deren Agenten-Kette.
    Open { path: String, claude: bool, origin: Option<crate::winctl::PeerChain> },
}

pub struct HookServer {
    pub port: u16,
    /// Token für Hook-Meldungen (wird in jedes Hub-Terminal exportiert).
    pub token: String,
    /// Eigener Token für Befehle an die laufende Instanz (nur in `ipc.json`, nie in Terminal-Umgebungen) –
    /// sonst könnte jedes Programm in einem Hub-Terminal neue Sitzungen öffnen lassen.
    pub ipc_token: String,
    pub rx: Receiver<HookMsg>,
    pub cmd_rx: Receiver<IpcCmd>,
    /// Anfragen des MCP-Servers (Agenten-Schnittstelle); werden in der UI-Schleife beantwortet.
    pub mcp_rx: Receiver<McpJob>,
}

/// Eine Anfrage des MCP-Servers an die UI-Schleife. Die Antwort geht über `reply` zurück an den wartenden
/// Verbindungs-Thread; ist dieser schon weg (Zeitüberschreitung), schlägt `send` fehl und die Anfrage wird verworfen.
pub struct McpJob {
    pub tool: String,
    pub args: Value,
    /// Sitzung, aus der der Agent nach eigener Angabe aufruft (Umgebungsvariable `PANELDECK_SESSION` der Brücke) –
    /// nur ein Hinweis, siehe `App::mcp_resolve_caller`.
    pub caller: Option<u64>,
    /// Prozessbaum des Absenders, vom Hub auf dem Verbindungs-Thread ermittelt (`winctl::resolve_peer`, seit 2.3.0, #152):
    /// TCP-Tabelle → Prozess geöffnet und lebend → Job Object einer Sitzung (#168), sonst Vorfahren mit Erzeugungszeiten.
    /// Ein `caller_pid` in der Anfrage wird
    /// ignoriert (ein Client könnte ihn fälschen). Nicht zu ermitteln = Anfrage abgewiesen, nie „extern“.
    pub peer: crate::winctl::PeerChain,
    /// `PANELDECK_SESSION_KEY` der Brücke (nur für die Zuordnung über `caller` in WSL).
    pub caller_key: Option<String>,
    pub reply: std::sync::mpsc::SyncSender<Result<Value, (String, String)>>,
}

impl HookServer {
    pub fn start(ctx: &egui::Context) -> Option<HookServer> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).ok()?;
        let port = listener.local_addr().ok()?.port();
        let token = random_hex();
        let ipc_token = random_hex();
        let (tx, rx) = channel();
        let (cmd_tx, cmd_rx) = channel();
        let (mcp_tx, mcp_rx) = channel::<McpJob>();
        let (tok, ipc_tok) = (token.clone(), ipc_token.clone());
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(s) = stream else { continue };
                // Jede Verbindung in eigenem Thread mit Gesamtfrist und Größenlimit – eine langsame oder stumme
                // Verbindung (Slowloris) darf andere Anfragen nicht blockieren. Gleichzeitige Verbindungen sind begrenzt.
                if ACTIVE.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= MAX_CONN {
                    ACTIVE.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                    continue;
                }
                let (tx, cmd_tx, mcp_tx, tok, ipc_tok, ctx) = (tx.clone(), cmd_tx.clone(), mcp_tx.clone(), tok.clone(), ipc_tok.clone(), ctx.clone());
                std::thread::spawn(move || {
                    handle_connection(s, &tok, &ipc_tok, &tx, &cmd_tx, &mcp_tx, &ctx);
                    ACTIVE.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                });
            }
        });
        Some(HookServer { port, token, ipc_token, rx, cmd_rx, mcp_rx })
    }
}

static ACTIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
const MAX_CONN: usize = 32;

fn handle_connection(mut s: TcpStream, token: &str, ipc_token: &str, tx: &std::sync::mpsc::Sender<HookMsg>, cmd_tx: &std::sync::mpsc::Sender<IpcCmd>, mcp_tx: &std::sync::mpsc::Sender<McpJob>, ctx: &egui::Context) {
    // Ein Werkzeugaufruf darf bis zu 64 Felder mit je 20 000 Zeichen tragen (bis zu 4 Byte je Zeichen); größere Anfragen sind zu groß.
    const MAX: usize = 512 * 1024;
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let _ = s.set_read_timeout(Some(Duration::from_millis(250)));
    let mut data: Vec<u8> = Vec::new();
    let mut too_big = false;
    let mut chunk = [0u8; 4096];
    loop {
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                // Zu große Anfragen werden weiter gelesen (und verworfen), damit der Absender nicht mit einem Verbindungsabbruch
                // statt der Antwort „too_large“ dasteht.
                if !too_big {
                    data.extend_from_slice(&chunk[..n]);
                    if data.len() > MAX {
                        too_big = true;
                        data = Vec::new();
                    }
                }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => return,
        }
        if std::time::Instant::now() > deadline {
            return;
        }
    }
    if too_big {
        let _ = s.write_all(mcp_err("too_large", crate::state::t("Die Anfrage ist zu groß", "The request is too large")).to_string().as_bytes());
        return;
    }
    let Ok(buf) = String::from_utf8(data) else { return };
    if buf.contains("\"mcp\"") {
        if let Ok(v) = serde_json::from_str::<Value>(&buf) {
            if v.get("cmd").and_then(|c| c.as_str()) == Some("mcp") {
                handle_mcp(&mut s, &v, ipc_token, mcp_tx, ctx);
                return;
            }
        }
    }
    if let Some(mut cmd) = parse_cmd(&buf, ipc_token) {
        if let IpcCmd::Open { origin, .. } = &mut cmd {
            // Absender sofort bestimmen; ist er nicht zu ermitteln (schon beendet …), wird nichts geöffnet (#152).
            match peer_chain(&s) {
                Some(p) => *origin = Some(p),
                None => {
                    let _ = s.write_all(b"refused");
                    return;
                }
            }
        }
        let _ = cmd_tx.send(cmd);
        let _ = s.write_all(b"ok");
        ctx.request_repaint();
    } else if let Some(msg) = parse_message(&buf, token) {
        let _ = tx.send(msg);
        ctx.request_repaint();
    }
}

/// Prozess am anderen Ende der Verbindung (vom Hub ermittelt, nicht vom Client angegeben).
fn peer_chain(s: &TcpStream) -> Option<crate::winctl::PeerChain> {
    crate::winctl::resolve_peer(s.peer_addr().ok()?, s.local_addr().ok()?)
}

fn parse_cmd(raw: &str, token: &str) -> Option<IpcCmd> {
    let v: Value = serde_json::from_str(raw).ok()?;
    if v.get("token")?.as_str()? != token {
        return None;
    }
    match v.get("cmd")?.as_str()? {
        "show" => Some(IpcCmd::Show),
        "open" => Some(IpcCmd::Open { path: v.get("path")?.as_str()?.to_string(), claude: v.get("claude").and_then(|c| c.as_bool()).unwrap_or(false), origin: None }),
        _ => None,
    }
}

/// Port und Token für zweite Instanzen im Datenordner ablegen.
pub fn write_ipc_file(dir: &Path, port: u16, token: &str) {
    let _ = crate::state::write_atomic(&dir.join("ipc.json"), json!({"port": port, "token": token}).to_string().as_bytes());
}

/// Befehl an eine bereits laufende Instanz mit demselben Datenordner schicken.
/// Gibt `true` zurück, wenn sie ihn bestätigt hat.
pub fn forward_to_running(dir: &Path, cmd: &IpcCmd) -> bool {
    let Ok(s) = std::fs::read_to_string(dir.join("ipc.json")) else { return false };
    let Ok(v) = serde_json::from_str::<Value>(&s) else { return false };
    let (Some(port), Some(token)) = (v["port"].as_u64(), v["token"].as_str()) else { return false };
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port as u16));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(500)) else { return false };
    let body = match cmd {
        IpcCmd::Show => json!({"token": token, "cmd": "show"}),
        IpcCmd::Open { path, claude, .. } => json!({"token": token, "cmd": "open", "path": path, "claude": claude}),
    };
    if stream.write_all(body.to_string().as_bytes()).is_err() {
        return false;
    }
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(1500)));
    let mut reply = String::new();
    let _ = stream.read_to_string(&mut reply);
    reply == "ok"
}

fn mcp_err(code: &str, message: &str) -> Value {
    json!({"ok": false, "error": {"code": code, "message": message}})
}

/// Eine MCP-Anfrage entgegennehmen: Token und Stufe prüfen, an die UI weiterreichen, Antwort zurückschreiben.
/// Die Prüfung der Stufe passiert hier im Hub (nicht erst in der Brücke), damit ein anderer Client sie nicht umgehen kann.
fn handle_mcp(s: &mut TcpStream, v: &Value, ipc_token: &str, mcp_tx: &std::sync::mpsc::Sender<McpJob>, ctx: &egui::Context) {
    use crate::mcp;
    // Absender sofort auf diesem Thread bestimmen (vor jeder Wartezeit), solange seine Verbindung besteht (#152).
    let peer = peer_chain(s);
    let mut reply = |r: Value| {
        let _ = s.write_all(r.to_string().as_bytes());
    };
    if v.get("token").and_then(|t| t.as_str()) != Some(ipc_token) {
        return reply(mcp_err("unauthorized", crate::state::t("Ungültiger Token", "Invalid token")));
    }
    let Some(tool) = v.get("tool").and_then(|t| t.as_str()) else {
        return reply(mcp_err("bad_args", crate::state::t("tool fehlt", "tool is missing")));
    };
    let level = mcp::current_level();
    let expert = mcp::expert_mode();
    if tool == "_level" {
        return reply(json!({"ok": true, "result": {"level": level.as_u8(), "expert": expert}}));
    }
    // Stufe und Modus (einfache Todo-Liste / Tickets) prüft der Hub selbst; die UI-Schleife prüft nach dem Warten noch einmal.
    if let Err((code, message)) = mcp::check_access(tool, level, expert) {
        return reply(mcp_err(&code, &message));
    }
    if mcp::ACTIVE.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= mcp::MAX_ACTIVE {
        mcp::ACTIVE.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        return reply(mcp_err("busy", crate::state::t("Zu viele gleichzeitige Anfragen", "Too many concurrent requests")));
    }
    let args = v.get("args").cloned().unwrap_or_else(|| json!({}));
    // Wartezeit: bei Werkzeugen mit eigener Frist deren Wert (+ Reserve), sonst kurz.
    let wait = args.get("timeout_secs").and_then(|t| t.as_u64()).map(|t| t.clamp(1, mcp::MAX_WAIT_SECS) + 10).unwrap_or(30);
    let caller = v.get("caller").and_then(|c| c.as_u64());
    let caller_key = v.get("caller_key").and_then(|c| c.as_str()).map(str::to_string);
    // Nie die Angabe des Clients: der Prozessbaum am anderen Ende der Verbindung (#152). Nicht zu ermitteln → abweisen.
    let Some(peer) = peer else {
        mcp::ACTIVE.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        return reply(mcp_err(
            "forbidden",
            crate::state::t(
                "Paneldeck kann den aufrufenden Prozess nicht bestimmen (schon beendet oder Verbindung weg) – die Anfrage wird abgewiesen.",
                "Paneldeck cannot identify the calling process (it has exited or its connection is gone) – the request is refused.",
            ),
        ));
    };
    let (rtx, rrx) = std::sync::mpsc::sync_channel(1);
    let out = if mcp_tx.send(McpJob { tool: tool.to_string(), args, caller, peer, caller_key, reply: rtx }).is_err() {
        mcp_err("internal", crate::state::t("Hub nicht bereit", "Hub not ready"))
    } else {
        ctx.request_repaint();
        match rrx.recv_timeout(Duration::from_secs(wait)) {
            Ok(Ok(result)) => json!({"ok": true, "result": result}),
            Ok(Err((code, message))) => mcp_err(&code, &message),
            Err(_) => mcp_err("timeout", crate::state::t("Der Hub hat nicht rechtzeitig geantwortet (läuft die Oberfläche noch?)", "The hub did not answer in time (is the UI still running?)")),
        }
    };
    mcp::ACTIVE.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    reply(out);
}

/// Werkzeugaufruf an die laufende Instanz (Brücke → Hub). Fehler als `(Code, Text)`.
pub fn call_mcp(dir: &Path, tool: &str, args: &Value, caller: Option<u64>, key: Option<&str>) -> Result<Value, (String, String)> {
    let down = || {
        let msg = format!(
            "{} {} {}",
            crate::state::t("Kein laufendes Paneldeck für den Datenordner", "No running Paneldeck found for the data folder"),
            dir.display(),
            crate::state::t(
                "(Paneldeck läuft nicht oder nutzt einen anderen Datenordner). Bitte Paneldeck starten oder die Brücke mit --data-dir/--profile auf den richtigen Ordner richten.",
                "(Paneldeck is not running or uses a different data folder). Please start Paneldeck or point the bridge to the right folder with --data-dir/--profile."
            )
        );
        ("hub_not_running".to_string(), msg)
    };
    let s = std::fs::read_to_string(dir.join("ipc.json")).map_err(|_| down())?;
    let v: Value = serde_json::from_str(crate::state::strip_bom(&s)).map_err(|_| down())?;
    let (Some(port), Some(token)) = (v["port"].as_u64(), v["token"].as_str()) else { return Err(down()) };
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port as u16));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(800)).map_err(|_| down())?;
    // `caller_pid` wertet der Hub seit 2.3.0 nicht mehr aus (er ermittelt den Absender selbst); `caller_key` belegt `caller`.
    let body = json!({"token": token, "cmd": "mcp", "tool": tool, "args": args, "caller": caller, "caller_key": key});
    stream.write_all(body.to_string().as_bytes()).map_err(|_| down())?;
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(crate::mcp::MAX_WAIT_SECS + 30)));
    let mut reply = String::new();
    stream.read_to_string(&mut reply).map_err(|e| ("io".to_string(), format!("{}: {e}", crate::state::t("Verbindung zum Hub abgebrochen", "Connection to the hub was interrupted"))))?;
    let r: Value = serde_json::from_str(&reply).map_err(|_| ("io".to_string(), crate::state::t("Ungültige Antwort des Hubs", "Invalid reply from the hub").to_string()))?;
    if r["ok"].as_bool() == Some(true) {
        Ok(r["result"].clone())
    } else {
        Err((r["error"]["code"].as_str().unwrap_or("error").to_string(), r["error"]["message"].as_str().unwrap_or("error").to_string()))
    }
}

fn parse_message(raw: &str, token: &str) -> Option<HookMsg> {
    let v: Value = serde_json::from_str(raw).ok()?;
    if v.get("token")?.as_str()? != token {
        return None;
    }
    let session = v.get("session")?.as_u64()?;
    let key = v.get("key").and_then(|k| k.as_str()).map(str::to_string);
    let p = v.get("payload").cloned().unwrap_or(Value::Null);
    let s = |k: &str| p.get(k).and_then(|x| x.as_str()).map(|x| x.to_string());
    Some(HookMsg {
        session,
        event: s("hook_event_name").unwrap_or_default(),
        kind: s("notification_type"),
        message: s("message"),
        claude_session: s("session_id"),
        transcript_path: s("transcript_path"),
        cwd: s("cwd"),
        key,
        source: s("source"),
        tool_name: s("tool_name"),
        agent_id: s("agent_id"),
        trigger: s("trigger"),
        reason: s("reason"),
    })
}

/// Felder des Ereignisses, die der Hub auswertet (kurze Texte; nie Prompts, Werkzeug-Eingaben oder -Ausgaben).
const KEEP: [&str; 11] = ["hook_event_name", "notification_type", "message", "tool_name", "session_id", "transcript_path", "cwd", "source", "agent_id", "trigger", "reason"];

/// Ereignis-JSON von Claude auf die Felder kürzen, die der Hub braucht.
fn slim_payload(payload: Value) -> Value {
    match payload {
        Value::Object(m) => Value::Object(m.into_iter().filter(|(k, v)| KEEP.contains(&k.as_str()) && v.is_string()).collect()),
        other => other,
    }
}

/// Wird mit `paneldeck.exe --hook` aufgerufen. Gibt immer 0 zurück, damit Claude nie blockiert wird.
pub fn run_hook_client() -> i32 {
    let get = |k: &str| std::env::var(k).ok();
    let (Some(port), Some(token), Some(session)) = (get(ENV_PORT), get(ENV_TOKEN), get(ENV_SESSION)) else {
        return 0; // nicht in einem Hub-Terminal gestartet
    };
    let (Ok(port), Ok(session)) = (port.parse::<u16>(), session.parse::<u64>()) else { return 0 };
    // Als Bytes lesen: ein einzelnes ungültiges UTF-8-Byte (z. B. in einer Werkzeug-Ausgabe) darf die Meldung nicht kosten.
    let mut input = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut input);
    let payload: Value = serde_json::from_str(&String::from_utf8_lossy(&input)).unwrap_or(Value::Null);
    // Nur die vom Hub benutzten Felder weiterreichen: Prompts, Tool-Eingaben und -Ausgaben sind oft > 64 KiB und
    // würden vom Server verworfen (→ Status/Fortsetzen/„Fertig“ gingen verloren).
    let payload = slim_payload(payload);
    // Schlüssel des Starts mitsenden (#260): ohne ihn verwirft der Hub die Meldung.
    send_event(port, &token, session, get(ENV_SESSION_KEY).as_deref(), payload);
    0
}

pub fn send_event(port: u16, token: &str, session: u64, key: Option<&str>, payload: Value) {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    if let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(800)) {
        let body = json!({"token": token, "session": session, "key": key, "payload": payload}).to_string();
        let _ = s.write_all(body.as_bytes());
        // Ende ausdrücklich melden: Der Hub wertet die Meldung erst am Ende der Verbindung aus.
        let _ = s.shutdown(std::net::Shutdown::Write);
    }
}

// ------------------------------------------------------------ Settings-Dateien

pub fn hook_command(exe: &Path) -> String {
    format!("\"{}\" --hook", exe.to_string_lossy().replace('\\', "/"))
}

fn hooks_value(cmd: &str) -> Value {
    let mut hooks = serde_json::Map::new();
    for ev in EVENTS {
        let mut group = json!({"hooks": [{"type": "command", "command": cmd}]});
        if matches!(ev, "PreToolUse" | "PostToolUse") {
            group["matcher"] = json!("*");
        }
        hooks.insert(ev.to_string(), json!([group]));
    }
    Value::Object(hooks)
}

/// Settings-Datei für `claude --settings <datei>` (nur Hooks).
pub fn write_session_settings(dir: &Path, exe: &Path) -> Option<PathBuf> {
    let path = dir.join("claude-hooks.json");
    let v = json!({ "hooks": hooks_value(&hook_command(exe)) });
    std::fs::write(&path, serde_json::to_string_pretty(&v).ok()?).ok()?;
    Some(path)
}

/// Wie `write_session_settings`, aber mit WSL-Pfad zur .exe (Aufruf über WSL-Interop).
pub fn write_session_settings_wsl(dir: &Path, exe: &Path, shell_id: &str) -> Option<PathBuf> {
    let path = dir.join("claude-hooks-wsl.json");
    let wsl_exe = crate::pathfmt::convert(shell_id, &exe.to_string_lossy());
    let cmd = format!("\"{wsl_exe}\" --hook");
    let v = json!({ "hooks": hooks_value(&cmd) });
    std::fs::write(&path, serde_json::to_string_pretty(&v).ok()?).ok()?;
    Some(path)
}

/// Benutzer-Einstellungen von Claude Code: `$CLAUDE_CONFIG_DIR/settings.json`, sonst `~/.claude/settings.json`. Mit
/// gesetztem `CLAUDE_CONFIG_DIR` liest Claude Code `~/.claude` nicht – dort eingetragene Hooks liefen nie, und weil sie
/// als „global eingetragen“ galten, bekamen die Sitzungen auch keine eigenen.
pub fn global_settings_path() -> Option<PathBuf> {
    settings_path_for(std::env::var_os("CLAUDE_CONFIG_DIR"), home_os())
}

fn home_os() -> Option<std::ffi::OsString> {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))
}

fn settings_path_for(config_dir: Option<std::ffi::OsString>, home: Option<std::ffi::OsString>) -> Option<PathBuf> {
    let dir = match config_dir.filter(|c| !c.is_empty()) {
        Some(c) => PathBuf::from(c),
        None => PathBuf::from(home?).join(".claude"),
    };
    Some(dir.join("settings.json"))
}

/// Alle Orte, an denen Paneldeck seine Hooks global eingetragen haben kann (Deinstallation): der geltende und – mit
/// `CLAUDE_CONFIG_DIR` – auch `~/.claude/settings.json`, wohin ältere Versionen immer schrieben.
pub fn global_settings_candidates() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    for p in [global_settings_path(), settings_path_for(None, home_os())].into_iter().flatten() {
        if !v.contains(&p) {
            v.push(p);
        }
    }
    v
}

fn is_hub_entry(group: &Value) -> bool {
    group["hooks"].as_array().is_some_and(|hs| {
        hs.iter().any(|h| {
            h["command"].as_str().is_some_and(|c| c.contains("--hook") && c.to_lowercase().contains("paneldeck"))
        })
    })
}

pub fn global_installed(path: &Path) -> bool {
    let Ok(s) = std::fs::read_to_string(path) else { return false };
    let Ok(v) = serde_json::from_str::<Value>(crate::state::strip_bom(&s)) else { return false };
    v["hooks"]["Stop"].as_array().is_some_and(|a| a.iter().any(is_hub_entry))
}

/// Nur lesend: Sind die Paneldeck-Hooks global eingetragen, aber nicht für jedes Ereignis, das diese Version auswertet
/// (von einer älteren Version eingetragen)? Hub-Sitzungen bekommen die Hooks dann zusätzlich per `--settings`; gleiche
/// Befehle führt Claude Code nur einmal aus.
pub fn global_incomplete(path: &Path) -> bool {
    let Ok(s) = std::fs::read_to_string(path) else { return false };
    let Ok(v) = serde_json::from_str::<Value>(crate::state::strip_bom(&s)) else { return false };
    EVENTS.iter().any(|ev| !v["hooks"][ev].as_array().is_some_and(|a| a.iter().any(is_hub_entry)))
}

/// `CLAUDE_CONFIG_DIR` aus den Umgebungsvariablen eines Profils (Zeilen `NAME=Wert`), falls gesetzt und nicht leer.
pub fn profile_config_dir(profile_env: &str) -> Option<&str> {
    profile_env.lines().filter_map(|l| l.split_once('=')).filter(|(k, _)| k.trim().eq_ignore_ascii_case("CLAUDE_CONFIG_DIR")).map(|(_, v)| v.trim()).rfind(|v| !v.is_empty())
}

/// Braucht eine Sitzung die Hooks per `--settings`, obwohl es globale gibt?
///
/// - `global`: Paneldeck-Hooks stehen in den Benutzer-Einstellungen, die der Hub sieht (`settings`). Ohne sie: immer.
/// - Das Profil der Sitzung setzt ein anderes `CLAUDE_CONFIG_DIR` (`profile_env`): Claude liest dort andere
///   Einstellungen, die globalen Hooks gelten für diese Sitzung nicht.
/// - `incomplete`: Die globalen Hooks stammen von einer älteren Version und decken nicht alle Ereignisse ab. Zusätzliche
///   Hooks per `--settings` nur, wenn sie dieselbe `paneldeck.exe` aufrufen (`foreign` = eine andere): Gleiche Befehle
///   führt Claude Code einmal aus, verschiedene beide – dann käme jede Meldung doppelt.
pub fn session_needs_settings(global: bool, incomplete: bool, foreign: bool, profile_env: &str, settings: Option<&Path>) -> bool {
    if !global {
        return true;
    }
    let norm = |p: &str| p.trim().trim_end_matches(['\\', '/']).replace('\\', "/").to_lowercase();
    if let Some(dir) = profile_config_dir(profile_env) {
        let same = settings.and_then(Path::parent).is_some_and(|g| norm(&g.to_string_lossy()) == norm(dir));
        if !same {
            return true;
        }
    }
    incomplete && !foreign
}

/// Nur lesend: zeigen die global eingetragenen Paneldeck-Hooks auf eine andere `paneldeck.exe` als `exe` (z. B. eine
/// ältere Kopie)? Dann deren Pfad. Solche Hooks senden keinen Schlüssel des Starts (#260) und zählen nur noch für den
/// Status; neu eintragen behebt das.
pub fn global_foreign_exe(path: &Path, exe: &Path) -> Option<String> {
    let s = std::fs::read_to_string(path).ok()?;
    let v = serde_json::from_str::<Value>(crate::state::strip_bom(&s)).ok()?;
    let norm = |c: &str| c.trim().replace('\\', "/").to_lowercase();
    let ours = norm(&hook_command(exe));
    let groups = v["hooks"]["Stop"].as_array()?;
    let cmds = groups.iter().filter(|g| is_hub_entry(g)).flat_map(|g| g["hooks"].as_array().into_iter().flatten()).filter_map(|h| h["command"].as_str());
    let mut foreign = None;
    for c in cmds {
        if !c.contains("--hook") || !c.to_lowercase().contains("paneldeck") {
            continue;
        }
        if norm(c) == ours {
            return None;
        }
        let t = c.trim();
        let exe_part = match t.strip_prefix('"') {
            Some(r) => r.split('"').next().unwrap_or(r),
            None => t.trim_end_matches("--hook").trim(),
        };
        foreign.get_or_insert_with(|| exe_part.to_string());
    }
    foreign
}

fn load_settings(path: &Path) -> Result<Value, String> {
    match std::fs::read_to_string(path) {
        Ok(s) if crate::state::strip_bom(&s).trim().is_empty() => Ok(json!({})),
        Ok(s) => serde_json::from_str(crate::state::strip_bom(&s)).map_err(|e| format!("{}: {e}", path.display())),
        // Nur eine wirklich fehlende Datei gilt als leer; bei anderen Fehlern (gesperrt, keine Rechte) NICHT überschreiben.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn store_settings(path: &Path, v: &Value) -> Result<(), String> {
    // Original genau einmal sichern (nicht bei jeder Installation überschreiben); schlägt das Backup fehl, wird abgebrochen.
    if path.exists() {
        let backup = path.with_extension("json.paneldeck-backup");
        if !backup.exists() {
            std::fs::copy(path, &backup).map_err(|e| format!("Backup fehlgeschlagen ({}): {e}", backup.display()))?;
        }
    }
    if let Some(p) = path.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    // Atomar schreiben: erst temporäre Datei, dann umbenennen.
    let tmp = path.with_extension("json.paneldeck-tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(v).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// Trägt die Hooks in die globale Claude-Konfiguration ein (bestehende Hooks bleiben erhalten).
pub fn install_global(path: &Path, exe: &Path) -> Result<(), String> {
    let mut v = load_settings(path)?;
    let Some(root) = v.as_object_mut() else {
        return Err(crate::state::t("settings.json ist kein JSON-Objekt", "settings.json is not a JSON object").into());
    };
    let ours = hooks_value(&hook_command(exe));
    let hooks = root.entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks.as_object_mut().ok_or(crate::state::t("\"hooks\" ist kein Objekt", "\"hooks\" is not an object"))?;
    for (ev, groups) in ours.as_object().into_iter().flatten() {
        let arr = hooks.entry(ev.clone()).or_insert_with(|| json!([]));
        let arr = arr.as_array_mut().ok_or(crate::state::t("Hook-Eintrag ist keine Liste", "Hook entry is not a list"))?;
        arr.retain(|g| !is_hub_entry(g));
        arr.extend(groups.as_array().into_iter().flatten().cloned());
    }
    store_settings(path, &v)
}

pub fn uninstall_global(path: &Path) -> Result<(), String> {
    let mut v = load_settings(path)?;
    // Nichts von Paneldeck darin: die Datei des Nutzers nicht anfassen (weder neu formatieren noch leere Listen entfernen).
    let ours = v["hooks"].as_object().is_some_and(|h| h.values().any(|arr| arr.as_array().is_some_and(|a| a.iter().any(is_hub_entry))));
    if !ours {
        return Ok(());
    }
    let mut hooks_empty = false;
    if let Some(hooks) = v.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        for (_, arr) in hooks.iter_mut() {
            if let Some(a) = arr.as_array_mut() {
                a.retain(|g| !is_hub_entry(g));
            }
        }
        hooks.retain(|_, arr| arr.as_array().map(|a| !a.is_empty()).unwrap_or(true));
        hooks_empty = hooks.is_empty();
    }
    if hooks_empty {
        if let Some(root) = v.as_object_mut() {
            root.remove("hooks");
        }
    }
    store_settings(path, &v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_roundtrip_and_status() {
        let ctx = egui::Context::default();
        let srv = HookServer::start(&ctx).unwrap();
        // falscher Token wird ignoriert
        send_event(srv.port, "falsch", 1, None, json!({"hook_event_name": "Stop"}));
        send_event(
            srv.port,
            &srv.token,
            7,
            Some("k7"),
            json!({"hook_event_name": "Notification", "notification_type": "permission_prompt", "message": "Claude needs permission", "session_id": "abc"}),
        );
        let m = srv.rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(m.session, 7);
        assert_eq!(m.key.as_deref(), Some("k7"));
        assert_eq!(m.claude_session.as_deref(), Some("abc"));
        assert_eq!(status_after(ClaudeStatus::Working, &mut TurnState::default(), &m), ClaudeStatus::Permission);
        assert!(srv.rx.recv_timeout(Duration::from_millis(200)).is_err());
    }

    fn ev(event: &str) -> HookMsg {
        HookMsg { event: event.into(), ..HookMsg::default() }
    }

    /// Eine ganze Runde samt Unter-Agenten, Zusammenfassen und Hintergrund-Agenten: Der Status folgt dem Haupt-Agenten.
    #[test]
    fn status_follows_the_main_agents_turn() {
        use ClaudeStatus::*;
        let mut t = TurnState::default();
        let mut s = Unknown;
        let mut step = |m: HookMsg, t: &mut TurnState| {
            s = status_after(s, t, &m);
            s
        };
        let sub = |event: &str| HookMsg { agent_id: Some("a1".into()), ..ev(event) };
        assert_eq!(step(HookMsg { source: Some("startup".into()), ..ev("SessionStart") }, &mut t), Idle);
        assert_eq!(step(ev("UserPromptSubmit"), &mut t), Working);
        assert!(t.open);
        // Unter-Agent in der Runde: sein Ende lässt den Haupt-Agenten weiterarbeiten
        assert_eq!(step(sub("PreToolUse"), &mut t), Working);
        assert_eq!(step(ev("SubagentStop"), &mut t), Working);
        // Automatisches Zusammenfassen mitten in der Runde: Claude arbeitet weiter – vorher „bereit“, und die
        // Warteschlange schickte den nächsten Prompt mitten in die Arbeit
        assert_eq!(step(HookMsg { trigger: Some("auto".into()), ..ev("PreCompact") }, &mut t), Working);
        assert_eq!(step(HookMsg { source: Some("compact".into()), ..ev("SessionStart") }, &mut t), Working);
        assert_eq!(step(ev("PostToolUse"), &mut t), Working);
        assert_eq!(step(ev("Stop"), &mut t), Idle);
        assert!(!t.open);
        // Hintergrund-Agent nach dem Ende der Runde: „arbeitet“, bis er fertig ist – dann wieder bereit (vorher blieb
        // „arbeitet“ stehen, weil dafür kein Stop kommt)
        assert_eq!(step(sub("PostToolUse"), &mut t), Working);
        assert!(!t.open, "der Haupt-Agent ist nicht in einer Runde");
        assert_eq!(step(ev("SubagentStop"), &mut t), Idle);
        // Der Haupt-Agent wird von einem Hintergrund-Ergebnis geweckt (kein UserPromptSubmit): seine Werkzeuge öffnen die Runde
        assert_eq!(step(ev("PreToolUse"), &mut t), Working);
        assert_eq!(step(ev("SubagentStop"), &mut t), Working);
        assert_eq!(step(ev("Stop"), &mut t), Idle);
        // `/compact` von Hand: „arbeitet“ während des Zusammenfassens, danach bereit
        assert_eq!(step(HookMsg { trigger: Some("manual".into()), ..ev("PreCompact") }, &mut t), Working);
        assert_eq!(t.tentative, Some(Idle), "bricht das Zusammenfassen ab, gilt wieder bereit");
        assert_eq!(step(HookMsg { source: Some("compact".into()), ..ev("SessionStart") }, &mut t), Idle);
        assert_eq!(t.tentative, None);
        // `/clear` und das Ende
        assert_eq!(step(ev("SessionEnd"), &mut t), Unknown);
        assert_eq!(step(HookMsg { source: Some("clear".into()), ..ev("SessionStart") }, &mut t), Idle);
        assert_eq!(step(ev("GibtEsNicht"), &mut t), Idle);
        // Automatisches Zusammenfassen als Erstes in einer Runde, die ohne Prompt begann: danach arbeitet Claude weiter
        assert_eq!(step(HookMsg { trigger: Some("auto".into()), ..ev("PreCompact") }, &mut t), Working);
        assert_eq!(step(HookMsg { source: Some("compact".into()), ..ev("SessionStart") }, &mut t), Working);
        assert_eq!(step(ev("Stop"), &mut t), Idle);
    }

    #[test]
    fn notifications_map_by_type_and_by_text_for_older_claude_versions() {
        use ClaudeStatus::*;
        let note = |kind: Option<&str>, text: &str| HookMsg { kind: kind.map(str::to_string), message: Some(text.into()), ..ev("Notification") };
        let after = |cur, m: &HookMsg| status_after(cur, &mut TurnState::default(), m);
        assert_eq!(after(Working, &note(Some("permission_prompt"), "x")), Permission);
        assert_eq!(after(Working, &note(Some("idle_prompt"), "x")), Idle);
        assert_eq!(after(Working, &note(Some("elicitation_dialog"), "x")), Input);
        // Reine Auskünfte ändern nichts – vorher „Eingabe“, bis zum nächsten Prompt
        for kind in ["auth_success", "elicitation_complete", "elicitation_response"] {
            assert_eq!(after(Idle, &note(Some(kind), "x")), Idle, "{kind}");
            assert_eq!(after(Working, &note(Some(kind), "x")), Working, "{kind}");
        }
        // Ohne Typ (ältere Claude-Versionen): nach dem Text
        assert_eq!(after(Working, &note(None, "Claude needs your permission to use Bash")), Permission);
        assert_eq!(after(Idle, &note(None, "Claude is waiting for your input")), Idle);
        assert_eq!(after(Working, &note(None, "etwas anderes")), Input);
        assert_eq!(permission_tool("Claude needs your permission to use Bash"), Some("Bash"));
        assert_eq!(permission_tool("Claude is waiting for your input"), None);
        // idle_prompt schließt die Runde
        let mut t = TurnState { open: true, ..TurnState::default() };
        assert_eq!(status_after(Working, &mut t, &note(Some("idle_prompt"), "x")), Idle);
        assert!(!t.open);
    }

    #[test]
    fn screen_hints_find_the_spinner_and_open_questions() {
        let working = "● Ich schaue nach.\n\n✻ Cogitating… (12s · ↓ 1.2k tokens · esc to interrupt)\n\n────────\n> \n────────\n  ? for shortcuts";
        assert_eq!(screen_hints(working), ScreenHints { spinner: true, dialog: false });
        // In einem schmalen Bereich bricht die Zeile um
        assert!(screen_hints("✻ Cogitating… (12s · esc to\ninterrupt)\n> ").spinner);
        let idle = "● Fertig.\n\n────────\n> \n────────\n  ? for shortcuts";
        assert_eq!(screen_hints(idle), ScreenHints::default());
        let ask = "Bash command\n\n  rm -rf build\n\nDo you want to proceed?\n❯ 1. Yes\n  2. No\n\nEsc to cancel · Tab to amend";
        assert_eq!(screen_hints(ask), ScreenHints { spinner: false, dialog: true });
        // Nur der untere Teil zählt für die Arbeitsanzeige: alter Text weiter oben täuscht nicht
        let old = format!("esc to interrupt\nDo you want to proceed?\n{}> ", "zeile\n".repeat(25));
        assert_eq!(screen_hints(&old), ScreenHints::default());
        assert_eq!(screen_hints(""), ScreenHints::default());
    }

    #[test]
    fn stale_working_needs_evidence_or_long_silence() {
        use ClaudeStatus::*;
        let turn = TurnState { open: true, ..TurnState::default() };
        let o = |secs: u64| Observed { since_event: Duration::from_secs(secs), quiet: Duration::from_secs(secs), ..Observed::default() };
        // Ein langes Werkzeug: keine Meldung, aber die Arbeitsanzeige läuft
        let busy = Observed { quiet: Duration::from_millis(300), screen: ScreenHints { spinner: true, dialog: false }, ..o(600) };
        assert_eq!(stale_status(Working, &turn, &busy), None);
        // Arbeitsanzeige zu sehen, aber still (Animation angehalten): bleibt „arbeitet“, auch mit Beleg
        let frozen = Observed { screen: ScreenHints { spinner: true, dialog: false }, turn_ended: true, ..o(600) };
        assert_eq!(stale_status(Working, &turn, &frozen), None);
        // Beleg aus dem Transkript: nach kurzer Ruhe bereit
        assert_eq!(stale_status(Working, &turn, &Observed { turn_ended: true, ..o(1) }), None);
        assert_eq!(stale_status(Working, &turn, &Observed { turn_ended: true, ..o(2) }), Some(Idle));
        // Die Meldung kam eben erst, das Terminal ist schon länger still: noch warten
        assert_eq!(stale_status(Working, &turn, &Observed { turn_ended: true, since_event: Duration::from_millis(500), ..o(30) }), None);
        // Ohne Beleg: erst nach langer Ruhe, und dann unbekannt – nie „bereit“
        assert_eq!(stale_status(Working, &turn, &o(19)), None);
        assert_eq!(stale_status(Working, &turn, &o(20)), Some(Unknown));
        assert_eq!(stale_status(Working, &turn, &Observed { quiet: Duration::from_secs(1), ..o(600) }), None, "es kommt noch Ausgabe");
        // Der Nutzer hat seit der letzten Meldung getippt (Esc) und es ist still: schon nach kurzer Zeit unbekannt
        let typed = |secs: u64| Observed { input_since: Some(Duration::from_secs(secs)), ..o(secs) };
        assert_eq!(stale_status(Working, &turn, &typed(4)), None);
        assert_eq!(stale_status(Working, &turn, &typed(5)), Some(Unknown));
        // … nicht, während Claude weiter ausgibt (Tippen während der Arbeit) oder die Arbeitsanzeige steht
        assert_eq!(stale_status(Working, &turn, &Observed { quiet: Duration::from_secs(1), ..typed(8) }), None);
        assert_eq!(stale_status(Working, &turn, &Observed { screen: ScreenHints { spinner: true, dialog: false }, ..typed(8) }), None);
        // Bereit fasst der Rückfall nie an, unbekannt nur nach einem eigenen „Meldungen blieben aus“
        for cur in [Idle, Unknown] {
            assert_eq!(stale_status(cur, &turn, &Observed { turn_ended: true, ..o(600) }), None);
        }
        // Der Beleg kommt später als die Frist: aus unbekannt wird doch noch bereit
        let stale = TurnState { stale: true, ..TurnState::default() };
        assert_eq!(stale_status(Unknown, &stale, &Observed { turn_ended: true, ..o(30) }), Some(Idle));
        assert_eq!(stale_status(Unknown, &stale, &o(3600)), None, "ohne Beleg nie");
        assert_eq!(stale_status(Unknown, &stale, &Observed { turn_ended: true, screen: ScreenHints { spinner: true, dialog: false }, ..o(30) }), None);
        // Jede Meldung beendet diesen Zustand (Claude ist dann wirklich beendet oder meldet sich wieder)
        let mut t = stale.clone();
        assert_eq!(status_after(Unknown, &mut t, &ev("SessionEnd")), Unknown);
        assert!(!t.stale);
        // Ein nur angenommenes „arbeitet“ wartet seine eigene Frist ab, auch wenn getippt wurde
        let assumed = TurnState { tentative: Some(Idle), ..TurnState::default() };
        assert_eq!(stale_status(Working, &assumed, &typed(6)), None);
        assert_eq!(stale_status(Working, &assumed, &typed(10)), Some(Idle));
    }

    #[test]
    fn a_stale_approval_question_needs_evidence_or_a_visible_answer() {
        use ClaudeStatus::*;
        let turn = TurnState { open: true, ..TurnState::default() };
        let o = |secs: u64| Observed { since_event: Duration::from_secs(secs), quiet: Duration::from_secs(secs), ..Observed::default() };
        for cur in [Permission, Input] {
            // Die Frage steht da und niemand antwortet: bleibt, egal wie lange
            assert_eq!(stale_status(cur, &turn, &o(3600)), None);
            // Im Terminal abgelehnt (kein Hook): das Transkript belegt das Ende der Runde
            assert_eq!(stale_status(cur, &turn, &Observed { turn_ended: true, ..o(2) }), Some(Idle));
            // Im Terminal erlaubt, das Werkzeug läuft lange: Eingabe + laufende Arbeitsanzeige + keine Rückfrage mehr
            let run = Observed { quiet: Duration::from_millis(200), screen: ScreenHints { spinner: true, dialog: false }, input_since: Some(Duration::from_secs(2)), ..o(30) };
            assert_eq!(stale_status(cur, &turn, &run), Some(Working));
            // … ohne Eingabe des Nutzers nie (andere Werkzeuge laufen weiter, die Frage steht noch)
            assert_eq!(stale_status(cur, &turn, &Observed { input_since: None, ..run }), None);
            // … nicht, solange eine Rückfrage zu sehen ist (die nächste Frage), und nicht ohne Arbeitsanzeige
            assert_eq!(stale_status(cur, &turn, &Observed { screen: ScreenHints { spinner: true, dialog: true }, ..run }), None);
            assert_eq!(stale_status(cur, &turn, &Observed { screen: ScreenHints::default(), ..run }), None);
            // … und nicht in dem Augenblick, in dem getippt wird
            assert_eq!(stale_status(cur, &turn, &Observed { input_since: Some(Duration::from_millis(200)), ..run }), None);
        }
    }

    /// Der Hook-Aufruf reicht nur kurze Textfelder weiter (und die neuen: Unter-Agent, Auslöser des Zusammenfassens).
    #[test]
    fn hook_client_forwards_only_the_fields_the_hub_uses() {
        let p = slim_payload(json!({
            "hook_event_name": "PostToolUse", "session_id": "A", "tool_name": "Bash", "agent_id": "a1", "trigger": "auto",
            "tool_input": {"command": "x".repeat(100_000)}, "tool_response": "y".repeat(100_000), "prompt": "geheim", "message": {"kein": "Text"}
        }));
        assert_eq!(p, json!({"hook_event_name": "PostToolUse", "session_id": "A", "tool_name": "Bash", "agent_id": "a1", "trigger": "auto"}));
        let raw = json!({"token": "t", "session": 3, "key": "k", "payload": p}).to_string();
        let m = parse_message(&raw, "t").unwrap();
        assert_eq!((m.tool_name.as_deref(), m.agent_id.as_deref(), m.trigger.as_deref()), (Some("Bash"), Some("a1"), Some("auto")));
        assert_eq!(slim_payload(Value::Null), Value::Null);
    }

    /// Jedes Ereignis, für das sich Paneldeck einträgt, kennt Claude Code mindestens so lange wie SessionEnd – ein
    /// unbekannter Name machte die Settings-Datei für ältere Versionen ungültig.
    #[test]
    fn only_long_known_hook_events_are_registered() {
        let known = ["PreToolUse", "PostToolUse", "Notification", "UserPromptSubmit", "Stop", "SubagentStop", "PreCompact", "SessionStart", "SessionEnd"];
        let v = hooks_value("x --hook");
        let names: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(names.len(), EVENTS.len());
        for n in &names {
            assert!(known.contains(n), "{n}");
        }
        for must in ["Stop", "SubagentStop", "PreCompact", "SessionEnd", "Notification"] {
            assert!(names.contains(&must), "{must}");
        }
        // Nur Werkzeug-Ereignisse tragen einen Matcher
        assert_eq!(v["PreToolUse"][0]["matcher"], "*");
        assert!(v["Stop"][0].get("matcher").is_none() && v["PreCompact"][0].get("matcher").is_none());
    }

    /// Globale Hooks einer älteren Version (ohne die neuen Ereignisse) werden erkannt – nur lesend.
    #[test]
    fn global_hooks_of_an_older_version_are_incomplete() {
        let dir = std::env::temp_dir().join(format!("hubtest-{}", random_hex()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("settings.json");
        assert!(!global_incomplete(&f), "keine Datei");
        let exe = Path::new("C:\\Tools\\paneldeck.exe");
        install_global(&f, exe).unwrap();
        assert!(global_installed(&f) && !global_incomplete(&f));
        // Stand einer älteren Version: SubagentStop und PreCompact fehlen
        let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        for old in ["SubagentStop", "PreCompact"] {
            v["hooks"].as_object_mut().unwrap().remove(old);
        }
        std::fs::write(&f, v.to_string()).unwrap();
        let before = std::fs::read(&f).unwrap();
        assert!(global_installed(&f) && global_incomplete(&f));
        assert_eq!(std::fs::read(&f).unwrap(), before, "nur lesen");
        // Neu eintragen ergänzt sie; Entfernen nimmt alle wieder heraus
        install_global(&f, exe).unwrap();
        assert!(!global_incomplete(&f));
        uninstall_global(&f).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert!(v.get("hooks").is_none(), "{v}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Review #282 (LOW-5, LOW-7): wann eine Sitzung die Hooks per `--settings` bekommt, obwohl es globale gibt.
    #[test]
    fn sessions_get_their_own_hooks_when_the_global_ones_do_not_apply() {
        let global = Path::new("C:\\Users\\x\\.claude\\settings.json");
        let needs = |g: bool, incomplete: bool, foreign: bool, env: &str| session_needs_settings(g, incomplete, foreign, env, Some(global));
        assert!(needs(false, false, false, ""), "ohne globale Hooks immer");
        assert!(!needs(true, false, false, ""), "vollständige globale Hooks reichen");
        // Globale Hooks einer älteren Version: zusätzlich per --settings – aber nur, wenn es dieselbe paneldeck.exe ist.
        // Bei einer anderen legt Claude Code die Befehle nicht zusammen, jede Meldung käme doppelt.
        assert!(needs(true, true, false, ""));
        assert!(!needs(true, true, true, ""));
        assert!(!needs(true, false, true, ""));
        // Das Profil setzt ein anderes CLAUDE_CONFIG_DIR: Claude liest die globalen Hooks dort nicht
        assert!(needs(true, false, false, "FOO=1\nCLAUDE_CONFIG_DIR=D:\\cfg\n"));
        assert!(needs(true, false, true, "claude_config_dir = D:/cfg"), "Name ohne Rücksicht auf Groß-/Kleinschreibung");
        // … dasselbe Verzeichnis (andere Schreibweise) oder leer: nicht
        assert!(!needs(true, false, false, "CLAUDE_CONFIG_DIR=c:/users/x/.claude/"));
        assert!(!needs(true, false, false, "CLAUDE_CONFIG_DIR=\nFOO=1"));
        assert!(session_needs_settings(true, false, false, "CLAUDE_CONFIG_DIR=D:\\cfg", None));
        assert_eq!(profile_config_dir("A=1\nCLAUDE_CONFIG_DIR=x\nCLAUDE_CONFIG_DIR=y"), Some("y"), "die letzte Zeile gilt");
        assert_eq!(profile_config_dir("A=1\nkeine Zuweisung"), None);
    }

    /// Review #282 (MEDIUM-2): Was die Warteschlange wissen muss – ob Claude wirklich endet und ob eine Runde mit einem
    /// abgeschickten Prompt begann.
    #[test]
    fn turn_and_session_end_tell_what_the_queue_may_do() {
        use ClaudeStatus::*;
        let end = |reason: Option<&str>| HookMsg { reason: reason.map(str::to_string), ..ev("SessionEnd") };
        assert!(session_end_is_exit(&end(None)) && session_end_is_exit(&end(Some("prompt_input_exit"))) && session_end_is_exit(&end(Some("logout"))) && session_end_is_exit(&end(Some("other"))));
        assert!(!session_end_is_exit(&end(Some("clear"))) && !session_end_is_exit(&end(Some("resume"))), "/clear und /resume: derselbe Claude läuft weiter");
        assert!(!session_end_is_exit(&HookMsg { reason: Some("other".into()), ..ev("Stop") }));
        let raw = json!({"token": "t", "session": 3, "key": "k", "payload": slim_payload(json!({"hook_event_name": "SessionEnd", "reason": "clear", "x": 1}))}).to_string();
        assert_eq!(parse_message(&raw, "t").unwrap().reason.as_deref(), Some("clear"));
        // Nur UserPromptSubmit öffnet eine Runde „mit Prompt“; Stop, SessionStart, idle_prompt und SessionEnd schließen sie
        let mut t = TurnState::default();
        status_after(Idle, &mut t, &ev("PreToolUse"));
        assert!(t.open && !t.prompted, "Werkzeug ohne Prompt (geweckte Runde)");
        status_after(Working, &mut t, &ev("UserPromptSubmit"));
        assert!(t.prompted);
        status_after(Working, &mut t, &HookMsg { source: Some("compact".into()), ..ev("SessionStart") });
        assert!(t.prompted, "automatisches Zusammenfassen mitten in der Runde");
        for close in [ev("Stop"), HookMsg { source: Some("clear".into()), ..ev("SessionStart") }, HookMsg { kind: Some("idle_prompt".into()), ..ev("Notification") }, ev("SessionEnd")] {
            let mut t = TurnState { open: true, prompted: true, ..TurnState::default() };
            status_after(Working, &mut t, &close);
            assert!(!t.prompted && !t.open, "{}", close.event);
        }
    }

    /// Mit `CLAUDE_CONFIG_DIR` liest Claude Code seine Einstellungen dort, nicht in `~/.claude`.
    #[test]
    fn global_settings_follow_claude_config_dir() {
        use std::ffi::OsString;
        let p = |c: Option<&str>, h: Option<&str>| settings_path_for(c.map(OsString::from), h.map(OsString::from));
        let home = PathBuf::from("C:\\Users\\x").join(".claude").join("settings.json");
        assert_eq!(p(None, Some("C:\\Users\\x")), Some(home.clone()));
        assert_eq!(p(Some(""), Some("C:\\Users\\x")), Some(home), "leer zählt nicht");
        assert_eq!(p(Some("D:\\cfg"), Some("C:\\Users\\x")), Some(PathBuf::from("D:\\cfg").join("settings.json")));
        assert_eq!(p(Some("D:\\cfg"), None), Some(PathBuf::from("D:\\cfg").join("settings.json")));
        assert_eq!(p(None, None), None);
    }

    /// Entfernen fasst eine Datei ohne Paneldeck-Hooks nicht an (kein Umformatieren, keine Sicherungskopie).
    #[test]
    fn uninstall_leaves_a_file_without_paneldeck_hooks_untouched() {
        let dir = std::env::temp_dir().join(format!("hubtest-{}", random_hex()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("settings.json");
        let own = "{\n\t\"model\": \"opus\",   \"hooks\": {\"Stop\": [], \"PreToolUse\": [{\"hooks\": [{\"type\": \"command\", \"command\": \"echo fremd\"}]}]}\n}\n";
        std::fs::write(&f, own).unwrap();
        uninstall_global(&f).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), own);
        assert!(!f.with_extension("json.paneldeck-backup").exists());
        // Eine fehlende Datei entsteht dabei nicht
        let missing = dir.join("nichts.json");
        uninstall_global(&missing).unwrap();
        assert!(!missing.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Die Shell steht wieder am Prompt: der bisherige Claude ist beendet, ein neuer darf übernehmen (auch ohne SessionEnd).
    #[test]
    fn a_new_claude_may_take_over_after_the_shell_prompt() {
        let msg = |ev: &str, id: &str, src: Option<&str>| HookMsg { event: ev.into(), claude_session: Some(id.into()), source: src.map(str::to_string), ..HookMsg::default() };
        let mut c = ConvBinding::launch(Some("A".into()), false);
        assert_eq!(c.feed(&msg("SessionStart", "A", Some("startup"))), ConvMatch::Bind("A".into()));
        assert_eq!(c.feed(&msg("SessionStart", "B", Some("startup"))), ConvMatch::Foreign, "claude -p, solange A läuft");
        c.shell_prompt();
        // Meldungen ohne Schlüssel sind danach nicht mehr sicher fremd
        assert!(!c.is_foreign(&msg("Stop", "B", None)));
        // Nur ein SessionStart übernimmt
        assert_eq!(c.feed(&msg("UserPromptSubmit", "B", None)), ConvMatch::Foreign);
        assert_eq!(c.feed(&msg("SessionStart", "B", Some("startup"))), ConvMatch::Switch("B".into()));
        assert!(!c.open);
        assert_eq!(c.feed(&msg("SessionStart", "C", Some("startup"))), ConvMatch::Foreign, "wieder streng");
        // Der Prompt hat getäuscht: A meldet sich weiter und bleibt zugeordnet
        let mut c = ConvBinding::launch(Some("A".into()), false);
        c.feed(&msg("SessionStart", "A", Some("startup")));
        c.shell_prompt();
        assert_eq!(c.feed(&msg("PreToolUse", "A", None)), ConvMatch::Same);
        assert_eq!(c.feed(&msg("SessionStart", "B", Some("startup"))), ConvMatch::Foreign);
        // Nichts zugeordnet (Claude startete nie): nach dem Prompt bindet ein SessionStart, die Start-ID weiterhin auch
        let mut c = ConvBinding::launch(Some("A".into()), false);
        c.shell_prompt();
        assert_eq!(c.feed(&msg("UserPromptSubmit", "A", None)), ConvMatch::Bind("A".into()));
        let mut c = ConvBinding::launch(Some("A".into()), false);
        c.shell_prompt();
        assert_eq!(c.feed(&msg("SessionStart", "B", Some("startup"))), ConvMatch::Bind("B".into()));
        // `--safe-mode` bleibt taub
        let mut c = ConvBinding::launch(Some("A".into()), true);
        c.shell_prompt();
        assert_eq!(c.feed(&msg("SessionStart", "B", Some("startup"))), ConvMatch::Foreign);
        assert!(c.is_foreign(&msg("Stop", "B", None)));
    }

    #[test]
    fn hook_messages_are_bound_to_the_launch() {
        // #260: nur der Schlüssel des laufenden Starts zählt
        assert!(key_matches("abcd", Some("abcd")));
        assert!(!key_matches("abcd", Some("abce")));
        assert!(!key_matches("abcd", Some("abc")));
        assert!(!key_matches("abcd", None), "alte/fremde Meldung ohne Schlüssel");
        assert!(!key_matches("", Some("")), "kein laufender Start");
        let msg = |ev: &str, id: Option<&str>, src: Option<&str>| HookMsg { event: ev.into(), claude_session: id.map(str::to_string), source: src.map(str::to_string), ..HookMsg::default() };
        let bind = |b: &str| ConvBinding { bound: Some(b.into()), ..ConvBinding::default() };
        assert_eq!(ConvBinding::default().feed(&msg("Stop", None, None)), ConvMatch::NoId);
        assert_eq!(ConvBinding::default().feed(&msg("SessionStart", Some("A"), Some("startup"))), ConvMatch::Bind("A".into()));
        assert_eq!(bind("A").feed(&msg("Stop", Some("A"), None)), ConvMatch::Same);
        // Verschachteltes `claude -p` mit geerbter Umgebung: eigene Konversation, wird ignoriert
        assert_eq!(bind("A").feed(&msg("SessionStart", Some("B"), Some("startup"))), ConvMatch::Foreign);
        assert_eq!(bind("A").feed(&msg("UserPromptSubmit", Some("B"), None)), ConvMatch::Foreign);
        assert_eq!(bind("A").feed(&msg("SessionEnd", Some("B"), None)), ConvMatch::Foreign);
        // /clear und /resume wechseln die Konversation
        assert_eq!(bind("A").feed(&msg("SessionStart", Some("C"), Some("clear"))), ConvMatch::Switch("C".into()));
        assert_eq!(bind("A").feed(&msg("SessionStart", Some("D"), Some("resume"))), ConvMatch::Switch("D".into()));
        // `source` und der Schlüssel werden vom Hook-Aufruf weitergereicht
        let raw = json!({"token": "t", "session": 3, "key": "k", "payload": {"hook_event_name": "SessionStart", "source": "clear", "session_id": "C"}}).to_string();
        let m = parse_message(&raw, "t").unwrap();
        assert_eq!((m.key.as_deref(), m.source.as_deref()), (Some("k"), Some("clear")));
    }

    /// Review zu #260: Claude endet und wird im selben Terminal neu gestartet – der neue Claude ordnet sich wieder zu.
    #[test]
    fn a_restarted_claude_in_the_same_shell_binds_again() {
        let msg = |ev: &str, id: &str, src: Option<&str>| HookMsg { event: ev.into(), claude_session: Some(id.into()), source: src.map(str::to_string), ..HookMsg::default() };
        let mut c = ConvBinding::launch(Some("A".into()), false);
        assert_eq!(c.feed(&msg("SessionStart", "A", Some("startup"))), ConvMatch::Bind("A".into()));
        assert_eq!(c.feed(&msg("Stop", "A", None)), ConvMatch::Same);
        // Ein `claude -p` aus der Sitzung heraus bleibt fremd, auch sein Ende löst nichts
        assert_eq!(c.feed(&msg("SessionStart", "N", Some("startup"))), ConvMatch::Foreign);
        assert_eq!(c.feed(&msg("SessionEnd", "N", None)), ConvMatch::Foreign);
        assert_eq!(c.bound.as_deref(), Some("A"));
        // Ende der eigenen Konversation: Zuordnung frei
        assert_eq!(c.feed(&msg("SessionEnd", "A", None)), ConvMatch::Same);
        assert_eq!(c, ConvBinding::default());
        // Vor einem neuen SessionStart zählen fremde Meldungen nicht
        assert_eq!(c.feed(&msg("UserPromptSubmit", "X", None)), ConvMatch::Foreign);
        // `claude` erneut im selben Terminal: neue ID, wird zugeordnet und bleibt es
        assert_eq!(c.feed(&msg("SessionStart", "B", Some("startup"))), ConvMatch::Bind("B".into()));
        assert_eq!(c.feed(&msg("UserPromptSubmit", "B", None)), ConvMatch::Same);
        assert_eq!(c.feed(&msg("SessionStart", "C", Some("startup"))), ConvMatch::Foreign);
        // /clear: SessionEnd der alten, SessionStart der neuen Konversation
        assert_eq!(c.feed(&msg("SessionEnd", "B", None)), ConvMatch::Same);
        assert_eq!(c.feed(&msg("SessionStart", "D", Some("clear"))), ConvMatch::Bind("D".into()));
    }

    /// Review zu #260: Meldungen ohne Schlüssel (globale Hooks einer älteren paneldeck.exe) zählen nur für den Status.
    #[test]
    fn keyless_messages_only_count_for_the_status() {
        let msg = |key: Option<&str>, id: Option<&str>| HookMsg { event: "Stop".into(), key: key.map(str::to_string), claude_session: id.map(str::to_string), ..HookMsg::default() };
        let c = ConvBinding::launch(Some("A".into()), false);
        assert_eq!(admit("k", false, &c, &msg(Some("k"), Some("A"))), Admit::Keyed);
        assert_eq!(admit("k", false, &c, &msg(Some("x"), Some("A"))), Admit::Drop, "falscher Schlüssel");
        assert_eq!(admit("k", false, &c, &msg(None, Some("A"))), Admit::StatusOnly);
        assert_eq!(admit("k", false, &c, &msg(None, None)), Admit::StatusOnly);
        // Fremde Konversation, schon eine Meldung mit Schlüssel, kein laufender Start, Hooks abgeschaltet: verwerfen
        assert_eq!(admit("k", false, &c, &msg(None, Some("B"))), Admit::Drop);
        assert_eq!(admit("k", true, &c, &msg(None, Some("A"))), Admit::Drop);
        assert_eq!(admit("", false, &c, &msg(None, Some("A"))), Admit::Drop);
        assert_eq!(admit("k", false, &ConvBinding::launch(None, true), &msg(None, Some("A"))), Admit::Drop);
        // Ohne bekannte Konversation (`--continue`): Status ja – zugeordnet wird dabei nichts
        let mut open = ConvBinding::launch(None, false);
        assert_eq!(admit("k", false, &open, &msg(None, Some("Z"))), Admit::StatusOnly);
        assert_eq!(open.bound, None);
        // Eine Meldung mit Schlüssel ordnet weiter normal zu
        assert_eq!(open.feed(&HookMsg { event: "SessionStart".into(), claude_session: Some("Z".into()), ..HookMsg::default() }), ConvMatch::Bind("Z".into()));
    }

    /// Review zu #260: nur die beim Start übergebene ID wird zugeordnet; mit `--safe-mode` nie eine aus den Hooks.
    #[test]
    fn only_the_launch_id_binds_and_safe_mode_takes_nothing() {
        let msg = |ev: &str, id: &str, src: Option<&str>| HookMsg { event: ev.into(), claude_session: Some(id.into()), source: src.map(str::to_string), ..HookMsg::default() };
        // `--session-id A`: ein `claude -p` (B), dessen Meldung zuerst ankommt, wird nicht zur Konversation der Sitzung
        let mut c = ConvBinding::launch(Some("A".into()), false);
        assert_eq!(c.feed(&msg("SessionStart", "B", Some("startup"))), ConvMatch::Foreign);
        assert_eq!(c.feed(&msg("UserPromptSubmit", "B", None)), ConvMatch::Foreign);
        assert_eq!(c.feed(&msg("UserPromptSubmit", "A", None)), ConvMatch::Bind("A".into()), "auch ohne SessionStart");
        // `--continue` (keine ID bekannt): nur SessionStart ordnet zu
        let mut c = ConvBinding::launch(None, false);
        assert_eq!(c.feed(&msg("UserPromptSubmit", "B", None)), ConvMatch::Foreign);
        assert_eq!(c.feed(&msg("SessionStart", "C", Some("resume"))), ConvMatch::Bind("C".into()));
        // `--safe-mode`: der Claude der Sitzung meldet nichts; was kommt, stammt von anderen Prozessen
        let mut c = ConvBinding::launch(Some("A".into()), true);
        for m in [msg("SessionStart", "B", Some("startup")), msg("SessionStart", "A", Some("startup")), msg("Stop", "B", None)] {
            assert_eq!(c.feed(&m), ConvMatch::Foreign);
            assert!(c.is_foreign(&m));
        }
        assert_eq!(c.bound, None);
        // Ohne Zustandsänderung (Meldungen ohne Schlüssel): fremd nur bei bekannter, abweichender Konversation
        let c = ConvBinding::launch(Some("A".into()), false);
        assert!(c.is_foreign(&msg("Stop", "B", None)) && !c.is_foreign(&msg("Stop", "A", None)));
        assert!(!ConvBinding::default().is_foreign(&msg("Stop", "B", None)));
        assert!(!c.is_foreign(&HookMsg { event: "Stop".into(), ..HookMsg::default() }));
    }

    #[test]
    fn global_install_keeps_foreign_hooks() {
        let dir = std::env::temp_dir().join(format!("hubtest-{}", random_hex()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("settings.json");
        std::fs::write(
            &f,
            r#"{"model":"opus","hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo fremd"}]}]}}"#,
        )
        .unwrap();
        let exe = Path::new("C:\\Tools\\paneldeck.exe");
        install_global(&f, exe).unwrap();
        install_global(&f, exe).unwrap(); // idempotent
        assert!(global_installed(&f));
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(v["model"], "opus");
        uninstall_global(&f).unwrap();
        assert!(!global_installed(&f));
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert!(v["hooks"].get("PreToolUse").is_none());
    }

    /// Review zu #260: globale Hooks einer anderen paneldeck.exe (ohne Schlüssel) werden erkannt – nur lesend.
    #[test]
    fn global_hooks_of_another_exe_are_detected() {
        let dir = std::env::temp_dir().join(format!("hubtest-{}", random_hex()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("settings.json");
        let exe = Path::new("C:\\Tools\\Paneldeck\\paneldeck.exe");
        assert_eq!(global_foreign_exe(&f, exe), None, "keine Datei");
        install_global(&f, exe).unwrap();
        assert_eq!(global_foreign_exe(&f, exe), None);
        // Groß-/Kleinschreibung und Trenner zählen unter Windows nicht
        assert_eq!(global_foreign_exe(&f, Path::new("c:/tools/paneldeck/PANELDECK.EXE")), None);
        let before = std::fs::read(&f).unwrap();
        assert_eq!(global_foreign_exe(&f, Path::new("D:\\Neu\\paneldeck.exe")).as_deref(), Some("C:/Tools/Paneldeck/paneldeck.exe"));
        assert_eq!(std::fs::read(&f).unwrap(), before, "nur lesen");
        // Neu eintragen behebt es
        install_global(&f, Path::new("D:\\Neu\\paneldeck.exe")).unwrap();
        assert_eq!(global_foreign_exe(&f, Path::new("D:\\Neu\\paneldeck.exe")), None);
        // Fremde Hooks ohne Paneldeck zählen nicht
        std::fs::write(&f, r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo fremd"}]}]}}"#).unwrap();
        assert_eq!(global_foreign_exe(&f, exe), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ipc_roundtrip() {
        let ctx = egui::Context::default();
        let srv = HookServer::start(&ctx).unwrap();
        let dir = std::env::temp_dir().join(format!("ipc-{}", random_hex()));
        std::fs::create_dir_all(&dir).unwrap();
        write_ipc_file(&dir, srv.port, &srv.ipc_token);
        // Der Hook-Token (steht in Terminal-Umgebungen) darf keine Befehle auslösen.
        write_ipc_file(&dir, srv.port, &srv.token);
        assert!(!forward_to_running(&dir, &IpcCmd::Show));
        write_ipc_file(&dir, srv.port, &srv.ipc_token);
        assert!(forward_to_running(&dir, &IpcCmd::Open { path: "C:\\x".into(), claude: true, origin: Some(crate::winctl::PeerChain { chain: vec![1], orphan: true, job: Some(9) }) }));
        // Der Hub ermittelt den Absender selbst (hier: dieser Testprozess), eine Angabe im Befehl zählt nicht.
        match srv.cmd_rx.recv_timeout(Duration::from_secs(2)).unwrap() {
            IpcCmd::Open { path, claude, origin } => {
                assert_eq!((path.as_str(), claude), ("C:\\x", true));
                assert_eq!(origin.as_ref().and_then(|o| o.chain.first()), Some(&std::process::id()));
                let origin = origin.unwrap();
                assert!(!origin.orphan);
                assert_eq!(origin.job, None, "die Job-Angabe des Clients zählt nicht; der Testprozess liegt in keinem Sitzungs-Job");
            }
            other => panic!("{other:?}"),
        }
        // veraltete Datei (falscher Port) → keine Bestätigung
        write_ipc_file(&dir, 1, "x");
        assert!(!forward_to_running(&dir, &IpcCmd::Show));
    }

    /// Zugriffsprüfung der Agenten-Schnittstelle im Hub (ohne UI): Token, Stufe, unbekanntes Werkzeug.
    /// Berührt die globale Stufe (`mcp::LEVEL`) – deshalb nur dieser eine Test.
    #[test]
    fn mcp_access_checks() {
        use crate::mcp::{McpLevel, EXPERT, LEVEL};
        use std::sync::atomic::Ordering;
        let ctx = egui::Context::default();
        let srv = HookServer::start(&ctx).unwrap();
        let dir = std::env::temp_dir().join(format!("mcp-{}", random_hex()));
        std::fs::create_dir_all(&dir).unwrap();
        write_ipc_file(&dir, srv.port, &srv.ipc_token);
        let code = |r: Result<Value, (String, String)>| r.err().map(|e| e.0).unwrap_or_default();
        // Standardsprache ist Englisch: die lesbaren Fehlermeldungen enthalten keine deutschen Zeichen.
        let msg = |r: Result<Value, (String, String)>| r.err().map(|e| e.1).unwrap_or_default();
        let english = |m: String| {
            assert!(!m.is_empty() && !m.chars().any(|c| "äöüÄÖÜß„".contains(c)), "not English: {m}");
        };
        // Stufe „Aus“: Stufenabfrage geht (für tools/list), alles andere nicht.
        LEVEL.store(McpLevel::Off.as_u8(), Ordering::Relaxed);
        assert_eq!(call_mcp(&dir, "_level", &json!({}), None, None).unwrap()["level"], 0);
        assert_eq!(code(call_mcp(&dir, "list_sessions", &json!({}), None, None)), "mcp_disabled");
        assert_eq!(code(call_mcp(&dir, "send_text", &json!({"session": 1, "text": "x"}), None, None)), "mcp_disabled");
        english(msg(call_mcp(&dir, "send_text", &json!({"session": 1, "text": "x"}), None, None)));
        // „Nur lesen“: Steuer-Werkzeuge sind verboten, unbekannte gibt es nicht.
        LEVEL.store(McpLevel::Read.as_u8(), Ordering::Relaxed);
        assert_eq!(call_mcp(&dir, "_level", &json!({}), None, None).unwrap()["level"], 1);
        assert_eq!(code(call_mcp(&dir, "run_command", &json!({"session": 1, "command": "dir"}), None, None)), "forbidden");
        assert_eq!(code(call_mcp(&dir, "close_session", &json!({"session": 1}), None, None)), "forbidden");
        assert_eq!(code(call_mcp(&dir, "gibt_es_nicht", &json!({}), None, None)), "not_found");
        english(msg(call_mcp(&dir, "gibt_es_nicht", &json!({}), None, None)));
        english(msg(call_mcp(&dir, "run_command", &json!({"session": 1, "command": "dir"}), None, None)));
        // Modus: Ticket-Werkzeuge gibt es nur im Expertenmodus, das Schreiben der Todo-Liste nur im einfachen Modus.
        LEVEL.store(McpLevel::Control.as_u8(), Ordering::Relaxed);
        EXPERT.store(false, Ordering::Relaxed);
        assert_eq!(call_mcp(&dir, "_level", &json!({}), None, None).unwrap()["expert"], false);
        assert_eq!(code(call_mcp(&dir, "create_ticket", &json!({"project": "PAN", "type": "x", "title": "t"}), None, None)), "wrong_mode");
        english(msg(call_mcp(&dir, "get_ticket", &json!({"ticket": "PAN-1"}), None, None)));
        EXPERT.store(true, Ordering::Relaxed);
        assert_eq!(call_mcp(&dir, "_level", &json!({}), None, None).unwrap()["expert"], true);
        assert_eq!(code(call_mcp(&dir, "add_todo", &json!({"title": "x"}), None, None)), "wrong_mode");
        assert_eq!(code(call_mcp(&dir, "set_todo_done", &json!({"id": 1}), None, None)), "wrong_mode");
        english(msg(call_mcp(&dir, "add_todo", &json!({"title": "x"}), None, None)));
        // die Stufe geht dem Modus vor: „Nur lesen“ meldet forbidden
        LEVEL.store(McpLevel::Read.as_u8(), Ordering::Relaxed);
        assert_eq!(code(call_mcp(&dir, "create_ticket", &json!({}), None, None)), "forbidden");
        EXPERT.store(false, Ordering::Relaxed);
        // Eine Anfrage über der Größengrenze bekommt den Fehler „too_large“ (statt eines abgerissenen Verbindungsaufbaus).
        LEVEL.store(McpLevel::Control.as_u8(), Ordering::Relaxed);
        EXPERT.store(true, Ordering::Relaxed);
        assert_eq!(code(call_mcp(&dir, "add_comment", &json!({"ticket": "PAN-1", "body": "x".repeat(2_000_000)}), None, None)), "too_large");
        // dagegen geht ein Kommentar mit der größten erlaubten Länge aus 4-Byte-Zeichen (20 000 Zeichen = 80 KB) bis in die UI-Schleife
        let big_ok = std::thread::spawn({
            let dir = dir.clone();
            move || call_mcp(&dir, "add_comment", &json!({"ticket": "PAN-1", "body": "😀".repeat(20_000)}), None, None)
        });
        let job = srv.mcp_rx.recv_timeout(Duration::from_secs(3)).expect("80 KB kommen an");
        assert_eq!(job.tool, "add_comment");
        assert_eq!(job.args["body"].as_str().map(|s| s.chars().count()), Some(20_000));
        job.reply.send(Ok(json!({"comment_id": 1}))).unwrap();
        assert_eq!(big_ok.join().unwrap().unwrap()["comment_id"], 1);
        LEVEL.store(McpLevel::Read.as_u8(), Ordering::Relaxed);
        EXPERT.store(false, Ordering::Relaxed);
        // Ein Lese-Werkzeug erreicht die UI-Schleife (hier: ein Test, der sie spielt).
        let reply = std::thread::spawn({
            let dir = dir.clone();
            move || call_mcp(&dir, "list_sessions", &json!({"include_exited": false}), Some(7), Some("k"))
        });
        let job = srv.mcp_rx.recv_timeout(Duration::from_secs(3)).expect("Anfrage kommt in der UI-Schleife an");
        // Absender: der Hub ermittelt den Prozess am anderen Ende selbst (dieser Testprozess).
        assert_eq!((job.tool.as_str(), job.caller, job.caller_key.as_deref()), ("list_sessions", Some(7), Some("k")));
        assert_eq!(job.peer.chain.first(), Some(&std::process::id()));
        assert!(!job.peer.orphan);
        assert_eq!(job.args["include_exited"], false);
        job.reply.send(Ok(json!({"sessions": []}))).unwrap();
        assert_eq!(reply.join().unwrap().unwrap()["sessions"], json!([]));
        // Falscher Token (der Hook-Token aus den Terminal-Umgebungen) wird abgewiesen.
        write_ipc_file(&dir, srv.port, &srv.token);
        assert_eq!(code(call_mcp(&dir, "list_sessions", &json!({}), None, None)), "unauthorized");
        english(msg(call_mcp(&dir, "list_sessions", &json!({}), None, None)));
        // Hub nicht erreichbar → verständlicher Fehler
        write_ipc_file(&dir, 1, "x");
        assert_eq!(code(call_mcp(&dir, "list_sessions", &json!({}), None, None)), "hub_not_running");
        english(msg(call_mcp(&dir, "list_sessions", &json!({}), None, None)));
        LEVEL.store(McpLevel::Off.as_u8(), Ordering::Relaxed);
        EXPERT.store(false, Ordering::Relaxed);
    }

    #[test]
    fn uuid_shape() {
        let u = new_uuid();
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
    }
}
