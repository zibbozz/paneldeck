//! Erkennung von Shell-Integrations-Sequenzen (OSC) im Ausgabestrom:
//!  - `ESC ] 133 ; A/B/D[;exit] BEL`  Prompt-Anfang/-Ende, Befehl beendet (FinalTerm/VS-Code/WezTerm-Konvention)
//!  - `ESC ] 9 ; 9 ; "pfad" BEL`      aktueller Ordner (ConEmu/Windows Terminal)
//!  - `ESC ] 7 ; file://host/pfad BEL` aktueller Ordner (OSC 7)
//!  - `ESC ] 7777 ; dur=<ms> BEL`      Dauer des letzten Befehls (eigene Erweiterung)
//!  - `ESC ] 9 ; 4 ; st ; pr BEL`      Fortschritt; `ESC ] 9 ; text BEL` und `ESC ] 777 ; notify ; titel ; text BEL`
//!    Benachrichtigungen eines Programms (#183, `vtnotify`; keine Shell-Integration)
//!  - `ESC [ ? 47/1047/1049/1000/1002/1003 h` Programm schaltet Vollbild oder Maus-Modus ein (kein OSC, aber für
//!    `Session::stale_modes`: ein `1049h` im schon aktiven alternativen Bildschirm ändert keine Modus-Bits)
//!  - `ESC [ 16 t`, `ESC [ > q` Anfragen, die die Emulation nicht beantwortet (Zellgröße, XTVERSION; `OscEvent::Query`)
//! Die Bytes werden zusätzlich unverändert an die Terminal-Emulation weitergegeben (unbekannte OSC werden dort ignoriert).

#[derive(Debug, Clone, PartialEq)]
pub enum OscEvent {
    PromptStart,
    CommandStart,
    CommandDone { exit: Option<i32> },
    Cwd(String),
    DurationMs(u64),
    /// DECSET für den alternativen Bildschirm (47/1047/1049) oder einen Maus-Modus (1000/1002/1003).
    /// Keine Shell-Integration: zählt nicht als `integration_seen`.
    FullscreenModeSet,
    /// Anfrage des Programms, die die Emulation (alacritty/vte) nicht selbst beantwortet (#183). Keine Shell-Integration.
    Query(Query),
    /// RIS (`ESC c`, z. B. `reset`): alacritty setzt dabei die von Programmen geänderten Farben nicht zurück, Paneldeck
    /// tut es selbst (wie xterm und Windows Terminal). Keine Shell-Integration.
    FullReset,
    /// Fortschritt (OSC 9;4, `None` = entfernen; siehe `vtnotify`). Keine Shell-Integration.
    Progress(Option<crate::vtnotify::Progress>),
    /// Benachrichtigung eines Programms (OSC 9 / OSC 777;notify), schon bereinigt. Keine Shell-Integration.
    Notify { title: String, body: String },
}

impl OscEvent {
    /// Meldung eines Programms (Fortschritt/Benachrichtigung, #183) statt der Shell-Integration.
    pub fn is_program_message(&self) -> bool {
        matches!(self, OscEvent::Progress(_) | OscEvent::Notify { .. })
    }

    /// Meldung der Shell-Integration (zählt für `integration_seen`)?
    pub fn is_integration(&self) -> bool {
        !matches!(self, OscEvent::FullscreenModeSet | OscEvent::Query(_) | OscEvent::FullReset) && !self.is_program_message()
    }
}

/// Anfragen, die Paneldeck selbst beantwortet (siehe `terminal::Reply`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Query {
    /// `CSI 16 t`: Zellgröße in Pixeln → `CSI 6 ; Höhe ; Breite t`.
    CellSizePx,
    /// `CSI > q` / `CSI > 0 q` (XTVERSION): Name und Version → `DCS > | Paneldeck <Version> ST`.
    XtVersion,
}

/// Private Modi, deren Einschalten ein Vollbildprogramm ankündigt (siehe `OscEvent::FullscreenModeSet`).
const FULLSCREEN_MODES: [u32; 6] = [47, 1047, 1049, 1000, 1002, 1003];
/// Länge der Parameter einer CSI-Sequenz, bis zu der sie ausgewertet wird (längere sind kein DECSET dieser Art).
const MAX_CSI: usize = 64;

#[derive(Default)]
enum State {
    #[default]
    Normal,
    Esc,
    Osc(Vec<u8>),
    OscEsc(Vec<u8>),
    /// `ESC [` gelesen: Parameter- und Zwischenbytes bis zum Endbyte.
    Csi(Vec<u8>),
}

#[derive(Default)]
pub struct OscScanner {
    state: State,
}

const MAX_PAYLOAD: usize = 4096;

impl OscScanner {
    #[cfg(test)]
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<OscEvent> {
        self.feed_at(bytes).0
    }

    /// Wie `feed`, dazu je Ereignis die Position im Chunk (Index hinter dem letzten Byte der Sequenz). Damit lässt sich
    /// der Chunk an den Ereignissen teilen und der Cursor genau dort auslesen (z. B. Prompt-Anfang).
    pub fn feed_at(&mut self, bytes: &[u8]) -> (Vec<OscEvent>, Vec<usize>) {
        let mut out = Vec::new();
        let mut pos: Vec<usize> = Vec::new();
        for (i, &b) in bytes.iter().enumerate() {
            let st = std::mem::take(&mut self.state);
            self.state = match st {
                State::Normal => {
                    if b == 0x1b {
                        State::Esc
                    } else {
                        State::Normal
                    }
                }
                State::Esc => esc_next(b, &mut out),
                State::Csi(mut buf) => match b {
                    // Parameter (0–9 ; : < = > ?) und Zwischenbytes
                    0x20..=0x3f => {
                        if buf.len() < MAX_CSI {
                            buf.push(b);
                            State::Csi(buf)
                        } else {
                            State::Normal
                        }
                    }
                    // Endbyte
                    0x40..=0x7e => {
                        if b == b'h' && is_fullscreen_decset(&buf) {
                            out.push(OscEvent::FullscreenModeSet);
                        } else if let Some(q) = query_of(&buf, b) {
                            out.push(OscEvent::Query(q));
                        }
                        State::Normal
                    }
                    0x1b => State::Esc,
                    // CAN/SUB brechen die Sequenz ab (wie vte).
                    0x18 | 0x1a => State::Normal,
                    // Andere C0-Steuerzeichen mitten in der Sequenz führt die Emulation aus, die Sequenz läuft weiter.
                    0x00..=0x1f => State::Csi(buf),
                    _ => State::Normal,
                },
                State::Osc(mut buf) => match b {
                    // BEL beendet das OSC; CAN/SUB ebenso (vte führt das OSC dabei aus und kehrt in den Grundzustand
                    // zurück, #221).
                    0x07 | 0x18 | 0x1a => {
                        parse(&buf, &mut out);
                        State::Normal
                    }
                    0x1b => State::OscEsc(buf),
                    // Übrige C0-Steuerzeichen überspringt vte im OSC (sie landen nicht im Inhalt).
                    0x00..=0x1f => State::Osc(buf),
                    _ => {
                        if buf.len() < MAX_PAYLOAD {
                            buf.push(b);
                            State::Osc(buf)
                        } else {
                            State::Normal
                        }
                    }
                },
                State::OscEsc(buf) => match b {
                    b'\\' => {
                        parse(&buf, &mut out);
                        State::Normal
                    }
                    // ESC beendet in vte das OSC und beginnt eine neue Sequenz (z. B. RIS direkt dahinter).
                    _ => esc_next(b, &mut out),
                },
            };
            // Ereignisse, die mit diesem Byte abgeschlossen wurden, bekommen seine Position.
            while pos.len() < out.len() {
                pos.push(i + 1);
            }
        }
        (out, pos)
    }
}

/// Höchstlänge des Inhalts einer OSC-Sequenz, den die Emulation zu sehen bekommt (#212). vte 0.15 puffert ein OSC
/// (mit `std`) ohne Grenze; ein nie beendetes OSC (`printf '\e]0;'; yes`) füllte den Speicher, bis der Hub abstürzt.
/// Groß genug für OSC 52 bis `terminal::OSC52_MAX_BYTES` (Base64 plus Kopf), damit dort weiter die Warnung kommt.
pub const OSC_LIMIT: usize = 2 * 1024 * 1024;

/// Kappt OSC-Sequenzen im Ausgabestrom, bevor vte sie sieht (wie `vtkeys::KittyStackGuard`). Verfolgt nur die
/// Zustände des vte-0.15-Parsers, die über Anfang und Ende eines OSC entscheiden. Über die Grenze hinaus wird der Inhalt
/// verworfen, bis das OSC endet (BEL, ESC, CAN/SUB gehen weiter an vte, das das gekürzte OSC dann ausführt). An der
/// Grenze steht einmal U+FFFD: Das gekürzte OSC ist damit ungültig statt unbemerkt abgeschnitten (ein gekürztes OSC 52
/// legte sonst einen Teil des Textes in die Zwischenablage).
pub struct OscLimit {
    state: LimitState,
    /// Bisher weitergegebene Bytes des Inhalts des laufenden OSC.
    len: usize,
    limit: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LimitState {
    /// Grundzustand und alle Sequenzen außer OSC (dort zählt nur, dass ESC/CAN/SUB sie beenden).
    Ground,
    Esc,
    /// `ESC` mit Zwischenbyte(s): ein folgendes `]` ist Endbyte, kein OSC.
    EscInter,
    Osc,
}

impl Default for OscLimit {
    fn default() -> Self {
        Self::with_limit(OSC_LIMIT)
    }
}

/// Ersatzzeichen an der Grenze eines gekappten OSC (U+FFFD).
const OSC_CUT: &[u8] = "\u{fffd}".as_bytes();

impl OscLimit {
    pub fn with_limit(limit: usize) -> Self {
        OscLimit { state: LimitState::Ground, len: 0, limit }
    }

    /// Ausgabe-Chunk ohne den Inhalt über der Grenze (meist unverändert; dann ohne Kopie).
    pub fn filter<'a>(&mut self, bytes: &'a [u8]) -> std::borrow::Cow<'a, [u8]> {
        let mut out: Option<Vec<u8>> = None;
        for (i, &b) in bytes.iter().enumerate() {
            match self.step(b) {
                LimitStep::Keep => {
                    if let Some(o) = out.as_mut() {
                        o.push(b);
                    }
                }
                LimitStep::Cut => out.get_or_insert_with(|| bytes[..i].to_vec()).extend_from_slice(OSC_CUT),
                LimitStep::Drop => {
                    out.get_or_insert_with(|| bytes[..i].to_vec());
                }
            }
        }
        match out {
            Some(o) => std::borrow::Cow::Owned(o),
            None => std::borrow::Cow::Borrowed(bytes),
        }
    }

    /// Ein Byte wie vte 0.15 (`Parser::advance`): ESC beginnt überall eine neue Sequenz, CAN/SUB kehren überall in den
    /// Grundzustand zurück (ein OSC wird dabei ausgeführt), BEL beendet ein OSC, übrige C0 überspringt vte im OSC.
    fn step(&mut self, b: u8) -> LimitStep {
        match b {
            0x18 | 0x1a => {
                self.state = LimitState::Ground;
                return LimitStep::Keep;
            }
            0x1b => {
                self.state = LimitState::Esc;
                return LimitStep::Keep;
            }
            _ => {}
        }
        match self.state {
            LimitState::Ground => {}
            LimitState::Esc => match b {
                b']' => {
                    self.state = LimitState::Osc;
                    self.len = 0;
                }
                // C0 führt vte aus, DEL und Bytes ab 0x80 übergeht es: der Zustand bleibt.
                0x00..=0x1f | 0x7f..=0xff => {}
                0x20..=0x2f => self.state = LimitState::EscInter,
                _ => self.state = LimitState::Ground,
            },
            LimitState::EscInter => {
                if (0x30..=0x7e).contains(&b) {
                    self.state = LimitState::Ground;
                }
            }
            LimitState::Osc => match b {
                0x07 => self.state = LimitState::Ground,
                0x00..=0x1f => {}
                _ if self.len < self.limit => self.len += 1,
                _ if self.len == self.limit => {
                    self.len += 1;
                    return LimitStep::Cut;
                }
                _ => return LimitStep::Drop,
            },
        }
        LimitStep::Keep
    }
}

enum LimitStep {
    Keep,
    /// Byte durch `OSC_CUT` ersetzen (erstes Byte über der Grenze).
    Cut,
    Drop,
}

/// Byte nach ESC (wie vte): `]` OSC, `[` CSI, `c` RIS; Steuerzeichen (außer CAN/SUB) und DEL lassen den Zustand
/// unverändert.
fn esc_next(b: u8, out: &mut Vec<OscEvent>) -> State {
    match b {
        b']' => State::Osc(Vec::new()),
        b'[' => State::Csi(Vec::new()),
        b'c' => {
            out.push(OscEvent::FullReset);
            State::Normal
        }
        0x18 | 0x1a => State::Normal,
        0x00..=0x1f | 0x7f => State::Esc,
        _ => State::Normal,
    }
}

/// `? Pm` (Parameter einer CSI-Sequenz mit Endbyte `h`) schaltet einen der `FULLSCREEN_MODES` ein?
fn is_fullscreen_decset(params: &[u8]) -> bool {
    let Some(rest) = params.strip_prefix(b"?") else { return false };
    let Ok(s) = std::str::from_utf8(rest) else { return false };
    s.split(';').any(|p| p.parse::<u32>().is_ok_and(|n| FULLSCREEN_MODES.contains(&n)))
}

/// CSI-Sequenz (Parameter `params`, Endbyte `fin`) als Anfrage, die Paneldeck selbst beantwortet.
fn query_of(params: &[u8], fin: u8) -> Option<Query> {
    match (params, fin) {
        (b"16", b't') => Some(Query::CellSizePx),
        (b">" | b">0", b'q') => Some(Query::XtVersion),
        _ => None,
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        // Auf Bytes arbeiten: `&s[i+1..i+3]` schnitte bei „%aé“ mitten in ein Zeichen (Panik → Absturz der App).
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = ((b[i + 1] as char).to_digit(16), (b[i + 2] as char).to_digit(16)) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn parse(payload: &[u8], out: &mut Vec<OscEvent>) {
    let Ok(s) = std::str::from_utf8(payload) else { return };
    if let Some(rest) = s.strip_prefix("133;") {
        let mut it = rest.split(';');
        match it.next() {
            Some("A") => out.push(OscEvent::PromptStart),
            Some("C") => out.push(OscEvent::CommandStart),
            Some("D") => out.push(OscEvent::CommandDone { exit: it.next().and_then(|c| c.trim().parse().ok()) }),
            _ => {}
        }
    } else if let Some(p) = s.strip_prefix("9;9;") {
        let p = p.trim().trim_matches('"');
        // Beliebige Programmausgabe kann diese Sequenz senden: keine Netzwerkpfade als Arbeitsordner übernehmen
        // (der Hub ruft dort regelmäßig git auf und löst Links relativ dazu auf).
        if !p.is_empty() && !crate::pathfmt::is_remote_unc(p) {
            out.push(OscEvent::Cwd(p.to_string()));
        }
    } else if let Some(u) = s.strip_prefix("7;file://") {
        // file://host/pfad  → /pfad (bei Laufwerken: /C:/… → C:/…)
        let path = u.find('/').map(|i| &u[i..]).unwrap_or("");
        let mut p = percent_decode(path);
        let b = p.as_bytes();
        if b.len() > 2 && b[0] == b'/' && b[2] == b':' && b[1].is_ascii_alphabetic() {
            p.remove(0);
        }
        if !p.is_empty() && !crate::pathfmt::is_remote_unc(&p) {
            out.push(OscEvent::Cwd(p));
        }
    } else if let Some(d) = s.strip_prefix("7777;dur=") {
        if let Ok(ms) = d.trim().parse() {
            out.push(OscEvent::DurationMs(ms));
        }
    } else if let Some(rest) = s.strip_prefix("9;4").filter(|r| r.is_empty() || r.starts_with(';')) {
        if let Some(p) = crate::vtnotify::parse_progress(rest) {
            out.push(OscEvent::Progress(p));
        }
    } else if let Some(text) = s.strip_prefix("9;") {
        // iTerm2-Benachrichtigung; `9;<zahl>;…` sind ConEmu-Befehle (u. a. 9;9 oben), die nie als Text erscheinen.
        if !crate::vtnotify::is_conemu_command(text) {
            push_note(out, "", text);
        }
    } else if let Some(rest) = s.strip_prefix("777;notify;") {
        let (title, body) = rest.split_once(';').unwrap_or((rest, ""));
        push_note(out, title, body);
    }
}

/// Bereinigte Benachrichtigung anhängen (leere fallen weg).
fn push_note(out: &mut Vec<OscEvent>, title: &str, body: &str) {
    let title = crate::vtnotify::sanitize(title, crate::vtnotify::TITLE_MAX);
    let body = crate::vtnotify::sanitize(body, crate::vtnotify::BODY_MAX);
    if !title.is_empty() || !body.is_empty() {
        out.push(OscEvent::Notify { title, body });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fullscreen_decset_is_detected() {
        let mut s = OscScanner::default();
        for seq in ["\x1b[?1049h", "\x1b[?47h", "\x1b[?1047h", "\x1b[?1000h", "\x1b[?1002h", "\x1b[?1003h", "\x1b[?1h\x1b[?25;1049;1006h"] {
            assert_eq!(s.feed(seq.as_bytes()), vec![OscEvent::FullscreenModeSet], "{seq:?}");
        }
        // Ausschalten, andere Modi, andere Sequenzen: nichts.
        for seq in ["\x1b[?1049l", "\x1b[?1000l", "\x1b[?1006h", "\x1b[?25h", "\x1b[1049h", "\x1b[?10490h", "\x1b[31m", "\x1b[2J", "\x1b[>1049h"] {
            assert!(s.feed(seq.as_bytes()).is_empty(), "{seq:?}");
        }
        // Über Chunk-Grenzen, mit Position hinter dem Endbyte, zusammen mit einem Prompt.
        assert!(s.feed_at(b"ab\x1b[?10").0.is_empty());
        let (ev, pos) = s.feed_at(b"49hxy\x1b]133;A\x07");
        assert_eq!(ev, vec![OscEvent::FullscreenModeSet, OscEvent::PromptStart]);
        assert_eq!(pos, vec![3, 13]);
        // Eine OSC-Sequenz mittendrin bleibt erkannt.
        assert_eq!(s.feed(b"\x1b[?1049\x1b]133;A\x07"), vec![OscEvent::PromptStart]);
        // Überlange Parameter brechen ab, danach wird wieder normal erkannt.
        let long = format!("\x1b[?{}1049h", "1;".repeat(100));
        assert!(s.feed(long.as_bytes()).is_empty());
        assert_eq!(s.feed(b"\x1b[?1049h"), vec![OscEvent::FullscreenModeSet]);
    }

    #[test]
    fn parses_sequences_across_chunks() {
        let mut s = OscScanner::default();
        let mut ev = s.feed(b"hallo \x1b]133;D;3\x07\x1b]9;9;\"C:\\Dev\\app\"\x07\x1b]13");
        ev.extend(s.feed(b"3;A\x1b\\text\x1b]7777;dur=1500\x07"));
        assert_eq!(
            ev,
            vec![
                OscEvent::CommandDone { exit: Some(3) },
                OscEvent::Cwd("C:\\Dev\\app".into()),
                OscEvent::PromptStart,
                OscEvent::DurationMs(1500)
            ]
        );
    }

    #[test]
    fn positions_mark_the_end_of_each_sequence() {
        let mut s = OscScanner::default();
        let data = b"ab\x1b]133;A\x07cd\x1b]133;D;0\x1b\\ef";
        let (ev, pos) = s.feed_at(data);
        assert_eq!(ev, vec![OscEvent::PromptStart, OscEvent::CommandDone { exit: Some(0) }]);
        // Position = Index direkt hinter dem Abschluss der Sequenz
        assert_eq!(&data[..pos[0]], b"ab\x1b]133;A\x07");
        assert_eq!(&data[pos[0]..pos[1]], b"cd\x1b]133;D;0\x1b\\");
        assert_eq!(&data[pos[1]..], b"ef");
        // Sequenz über Chunk-Grenzen: Position im Chunk, in dem sie endet
        let (ev, pos) = (OscScanner::default(), ()).0.feed_at(b"x");
        assert!(ev.is_empty() && pos.is_empty());
        let mut s = OscScanner::default();
        assert!(s.feed_at(b"\x1b]133;").0.is_empty());
        let (ev, pos) = s.feed_at(b"A\x07zz");
        assert_eq!(ev, vec![OscEvent::PromptStart]);
        assert_eq!(pos, vec![2]);
    }

    #[test]
    fn osc7_urls() {
        let mut s = OscScanner::default();
        let ev = s.feed(b"\x1b]7;file://PC/C:/Users/Max%20M/proj\x07\x1b]7;file:///home/max/a\x07");
        assert_eq!(ev, vec![OscEvent::Cwd("C:/Users/Max M/proj".into()), OscEvent::Cwd("/home/max/a".into())]);
    }

    #[test]
    fn hostile_cwd_sequences() {
        let mut s = OscScanner::default();
        // Mehrbyte-Zeichen direkt nach „%x“ darf nicht abstürzen
        let ev = s.feed("\x1b]7;file://h/C:/a%a\u{e9}b%\u{20ac}\x07".as_bytes());
        assert_eq!(ev.len(), 1);
        // Netzwerkpfade werden ignoriert
        assert!(s.feed(b"\x1b]9;9;\\\\evil\\share\x07\x1b]7;file://h//evil/share\x07").is_empty());
    }

    #[test]
    fn queries_are_detected() {
        let mut s = OscScanner::default();
        assert_eq!(s.feed(b"\x1b[16t"), vec![OscEvent::Query(Query::CellSizePx)]);
        assert_eq!(s.feed(b"\x1b[>q\x1b[>0q"), vec![OscEvent::Query(Query::XtVersion), OscEvent::Query(Query::XtVersion)]);
        // Über eine Chunk-Grenze, mit Position hinter dem Endbyte.
        assert!(s.feed_at(b"ab\x1b[>").0.is_empty());
        let (ev, pos) = s.feed_at(b"qcd");
        assert_eq!((ev, pos), (vec![OscEvent::Query(Query::XtVersion)], vec![1]));
        // Ähnliche Sequenzen sind keine Anfragen: 14t/18t beantwortet die Emulation, DECSCUSR, DA2, Fenster-Befehle.
        for seq in ["\x1b[14t", "\x1b[18t", "\x1b[6 q", "\x1b[>c", "\x1b[>1q", "\x1b[16;1t", "\x1b[116t", "\x1b[q"] {
            assert!(s.feed(seq.as_bytes()).is_empty(), "{seq:?}");
        }
        assert!(!OscEvent::Query(Query::CellSizePx).is_integration());
        assert_eq!(s.feed(b"x\x1bcy"), vec![OscEvent::FullReset]);
        // mit Steuerzeichen zwischen ESC und c, direkt hinter einem mit ESC beendeten OSC
        assert_eq!(s.feed(b"\x1b\x07\nc"), vec![OscEvent::FullReset]);
        assert_eq!(s.feed(b"\x1b]0;t\x1bc"), vec![OscEvent::FullReset]);
        assert!(s.feed(b"\x1b\x18c").is_empty());
        assert!(!OscEvent::FullReset.is_integration());
        // CAN/SUB brechen eine CSI-Sequenz ab wie in vte: danach ist `16t` keine Anfrage mehr.
        assert!(s.feed(b"\x1b[1\x186t").is_empty());
        assert!(s.feed(b"\x1b[>\x1aq").is_empty());
        assert!(!OscEvent::FullscreenModeSet.is_integration());
        assert!(OscEvent::PromptStart.is_integration());
    }

    #[test]
    fn progress_and_notifications() {
        use crate::vtnotify::{Progress, ProgressState};
        let mut s = OscScanner::default();
        let ev = s.feed(b"\x1b]9;4;1;42\x07\x1b]9;4;0;0\x1b\\\x1b]9;4;3\x07\x1b]9;4;9;1\x07");
        assert_eq!(
            ev,
            vec![
                OscEvent::Progress(Some(Progress { state: ProgressState::Normal, percent: 42 })),
                OscEvent::Progress(None),
                OscEvent::Progress(Some(Progress { state: ProgressState::Indeterminate, percent: 0 })),
            ]
        );
        let note = |t: &str, b: &str| OscEvent::Notify { title: t.into(), body: b.into() };
        let ev = s.feed("\x1b]9;Build fertig\x07\x1b]777;notify;Tests;3 rot\u{202e}\x07\x1b]777;notify;Nur Titel\x07".as_bytes());
        assert_eq!(ev, vec![note("", "Build fertig"), note("Tests", "3 rot"), note("Nur Titel", "")]);
        // ConEmu-Befehle sind keine Benachrichtigungen; 9;9 bleibt der Ordner, 9;40 ist kein Fortschritt.
        let ev = s.feed(b"\x1b]9;1;500\x07\x1b]9;7;calc\x07\x1b]9;9;C:\\x\x07\x1b]9;40\x07\x1b]777;preexec\x07\x1b]777;notify;;\x07");
        assert_eq!(ev, vec![OscEvent::Cwd("C:\\x".into())]);
        assert!(OscEvent::Progress(None).is_program_message() && !OscEvent::PromptStart.is_program_message());
    }

    /// CAN/SUB beenden ein OSC wie in vte (das es dabei ausführt); andere C0-Zeichen überspringt vte im OSC (#221).
    #[test]
    fn can_sub_end_osc_like_vte() {
        let mut s = OscScanner::default();
        assert_eq!(s.feed(b"\x1b]133;A\x18PS> "), vec![OscEvent::PromptStart]);
        assert_eq!(s.feed(b"\x1b]133;D;0\x1a"), vec![OscEvent::CommandDone { exit: Some(0) }]);
        // Nach CAN ist der Rest Text: ein späteres BEL beendet kein OSC mehr.
        let note = OscEvent::Notify { title: String::new(), body: "hi".into() };
        assert_eq!(s.feed(b"\x1b]9;hi\x18;rest\x07"), vec![note]);
        // Steuerzeichen im OSC zählen nicht zum Inhalt.
        assert_eq!(s.feed(b"\x1b]133;\r\nA\x07"), vec![OscEvent::PromptStart]);
        // Über eine Chunk-Grenze, Position hinter CAN.
        assert!(s.feed_at(b"\x1b]133;").0.is_empty());
        assert_eq!(s.feed_at(b"A\x18x"), (vec![OscEvent::PromptStart], vec![2]));
        // Mit vte verglichen: dieselbe Sequenz wird dort ebenfalls ausgeführt (OSC 2 = Titel).
        use alacritty_terminal::event::{Event, EventListener};
        use alacritty_terminal::term::{test::TermSize, Config, Term};
        use alacritty_terminal::vte::ansi::Processor;
        #[derive(Clone)]
        struct Titles(std::sync::Arc<std::sync::Mutex<Vec<String>>>);
        impl EventListener for Titles {
            fn send_event(&self, ev: Event) {
                if let Event::Title(t) = ev {
                    self.0.lock().unwrap().push(t);
                }
            }
        }
        let titles = Titles(Default::default());
        let mut term = Term::new(Config::default(), &TermSize::new(20, 5), titles.clone());
        let mut p: Processor = Processor::new();
        p.advance(&mut term, b"\x1b]2;a\x01b\x18c");
        assert_eq!(*titles.0.lock().unwrap(), vec!["ab".to_string()]);
    }

    /// Ein nie beendetes OSC bleibt für vte begrenzt (#212); kurze OSC laufen unverändert durch.
    #[test]
    fn osc_limit_caps_unterminated_osc() {
        let mut g = OscLimit::with_limit(16);
        let normal = b"\x1b]0;Titel\x07text\x1b]133;A\x1b\\\x1b[31m]x";
        assert!(matches!(g.filter(normal), std::borrow::Cow::Borrowed(_)));
        // `printf '\e]0;'; yes`: der Inhalt endet nach der Grenze, Zeilenvorschübe (C0) bleiben, vte überspringt sie.
        let mut fed = g.filter(b"\x1b]0;").into_owned();
        for _ in 0..10_000 {
            fed.extend_from_slice(&g.filter(b"y\n"));
        }
        let payload: Vec<u8> = fed[2..].iter().copied().filter(|b| *b >= 0x20).collect();
        assert_eq!(payload, [b"0;".as_slice(), &[b'y'; 14], OSC_CUT].concat());
        // Das Ende des OSC geht durch, danach ist wieder alles Text.
        assert_eq!(&*g.filter(b"yy\x07after"), b"\x07after");
        // ESC beendet das OSC ebenso; ein neues OSC zählt von vorn.
        let long = [b"\x1b]2;".as_slice(), &[b'z'; 40], b"\x1b\\ok"].concat();
        assert_eq!(&*g.filter(&long), [b"\x1b]2;".as_slice(), &[b'z'; 14], OSC_CUT, b"\x1b\\ok"].concat());
        // `ESC ( ]` ist kein OSC (Zwischenbyte), `]` im Text auch nicht.
        let other = [b"\x1b(]".as_slice(), &[b'q'; 40]].concat();
        assert!(matches!(g.filter(&other), std::borrow::Cow::Borrowed(_)));
        // CAN beendet das OSC.
        let can = [b"\x1b]2;".as_slice(), &[b'z'; 30], b"\x18", &[b'z'; 30]].concat();
        assert_eq!(&*g.filter(&can), [b"\x1b]2;".as_slice(), &[b'z'; 14], OSC_CUT, b"\x18", &[b'z'; 30]].concat());
        // Die echte Grenze reicht für OSC 52 bis zur Höchstgröße der Zwischenablage (Base64).
        assert!(OSC_LIMIT > crate::terminal::OSC52_MAX_BYTES.div_ceil(3) * 4 + 16);
    }

    #[test]
    fn ignores_garbage_and_other_osc() {
        let mut s = OscScanner::default();
        assert!(s.feed(b"\x1b[31mfarbig\x1b[0m \x1b]0;Titel\x07 \x1b]133;B\x07").is_empty());
    }
}
