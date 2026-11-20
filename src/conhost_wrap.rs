//! Ausgleich für die ConPTY von Windows (conhost mit dem alten VT-Renderer) nach einer Zeile in genau Terminalbreite
//! (#264, Folge von #231).
//!
//! Endet ein Frame von conhost mit dem Cursor am rechten Rand („Umbruch ausstehend“), nimmt sein Renderer danach an,
//! das Terminal stehe schon am Anfang der nächsten Zeile. Ein `\r\n` im nächsten Frame meint dann „noch eine Zeile
//! weiter“ – ein xterm-artiges Terminal (alacritty) hebt mit dem CR aber nur den ausstehenden Umbruch auf und landet
//! eine Zeile zu hoch. Typisch bei Git Bash: `printf` füllt die Zeile, die Leerzeile vor dem Prompt fehlt, und das
//! Terminal bleibt eine Zeile hinter conhost, bis neu gezeichnet wird (doppelte Echo-Zeilen bei Cursorsprüngen).
//!
//! Mitgeschnitten (Windows 11 26200, Git Bash, 40 Spalten, `POS` = Cursor laut conhost, CPR):
//! - `W` (40 Zeichen), Pause, `\r\n\r\n`, Pause, `Y` → ConPTY schickt `W` | `\r\nY`, conhost hat `Y` zwei Zeilen tiefer.
//! - `W\r\nY` ohne Pause (ein Frame) → ConPTY schickt `W\r\nY`, conhost hat `Y` eine Zeile tiefer (normal).
//! - In der untersten Zeile rollt conhost anders (CR LF, dann Neuzeichnen des letzten Zeichens): dort normal.
//! - Andere Ziele erreicht der Renderer per CUP (absolut), unabhängig vom Umbruch.
//!
//! Deshalb hier eng begrenzt: Nur wenn ein Lesestück (= Frame; ein kurzes Lesen heißt, die Pipe war leer) mit
//! ausstehendem Umbruch außerhalb der untersten Zeile endet und bis zum nächsten CR nichts außer Farben, ein paar
//! harmlosen Modi und OSC-Meldungen (Shell-Integration, Titel) kommt, bricht das Terminal vor diesem CR um.
//! Die mitgelieferte ConPTY (OpenConsole 1.25) und neuere conhost-Versionen zeichnen richtig; sie fragen beim Start
//! die Geräteattribute ab (DA1, `ESC [ c`), der alte Renderer nie – daran wird die Korrektur wieder abgeschaltet.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Point;
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{Handler, Processor};

/// So viele Bytes vom Anfang der Ausgabe werden nach der DA1-Anfrage eines neueren conhost durchsucht.
const STARTUP_PROBE: usize = 64;

/// Modi, die ein Frame zwischen der vollen Zeile und dem CR setzen darf (Cursor sichtbar, Bracketed Paste, Fokus,
/// win32-input-mode, Blinken) – sie bewegen den Cursor nicht.
const QUIET_MODES: &[&[u8]] = &[b"?25", b"?2004", b"?1004", b"?9001", b"?12"];

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Scan {
    #[default]
    Ground,
    Esc,
    Csi,
    /// In einer Zeichenkette (OSC, DCS, SOS, PM, APC): bis BEL bzw. ST.
    Str,
    StrEsc,
}

/// Wirkung eines Bytes auf die Bereitschaft.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Effect {
    /// Bewegt den Cursor nicht (Teil einer Sequenz, Farbe, harmloser Modus, OSC, BEL).
    Quiet,
    /// CR im Grundzustand.
    Cr,
    /// Alles andere (Text, Cursorbewegung, andere Steuerzeichen): Bereitschaft verfällt.
    Other,
}

/// Zustand der Korrektur je Sitzung (Lese-Thread).
#[derive(Debug, Default)]
pub struct ConhostWrap {
    enabled: bool,
    /// Cursor (mit ausstehendem Umbruch) am Ende des letzten Frames.
    armed: Option<Point>,
    scan: Scan,
    /// Parameter der laufenden CSI-Sequenz (gekürzt).
    params: Vec<u8>,
    /// Anfang der Ausgabe, bis entschieden ist, ob ein neuerer conhost zeichnet.
    probe: Option<Vec<u8>>,
}

impl ConhostWrap {
    /// `system` = die Sitzung läuft auf der ConPTY von Windows (`console_host == "system"`).
    pub fn new(system: bool) -> Self {
        ConhostWrap { enabled: system, probe: system.then(Vec::new), ..Default::default() }
    }

    /// Ist die Korrektur (noch) aktiv?
    #[cfg(test)]
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Wie `advance` (über `feed`), aber vor einem CR, der in conhost „eine Zeile weiter“ heißt, umbrechen.
    /// `feed` bekommt die Bytes in Stücken (Verlaufszählung, offene Prompt-Marken); `wrap` bricht um (CR + LF direkt
    /// an der Emulation, ohne den Parser zu berühren).
    pub fn advance<T: EventListener>(
        &mut self,
        p: &mut Processor,
        t: &mut Term<T>,
        bytes: &[u8],
        mut feed: impl FnMut(&mut Processor, &mut Term<T>, &[u8]),
        mut wrap: impl FnMut(&mut Term<T>, &mut dyn FnMut(&mut Term<T>)),
    ) {
        if self.enabled {
            self.probe_startup(bytes);
        }
        if !self.enabled {
            feed(p, t, bytes);
            return;
        }
        let mut start = 0;
        for (i, &b) in bytes.iter().enumerate() {
            match self.step(b) {
                Effect::Quiet => {}
                Effect::Other => self.armed = None,
                Effect::Cr => {
                    let Some(at) = self.armed.take() else { continue };
                    feed(p, t, &bytes[start..i]);
                    start = i;
                    if p.sync_bytes_count() == 0 && pending_wrap_at(t) == Some(at) {
                        wrap(t, &mut |t: &mut Term<T>| {
                            t.carriage_return();
                            t.linefeed();
                        });
                    }
                }
            }
        }
        feed(p, t, &bytes[start..]);
    }

    /// Vor einem Lesestück (dem ganzen, auch wenn `advance` es in Teilen bekommt): Rollt conhost in diesem Frame unter
    /// der vollen Zeile weiter, rechnet es wie xterm (CR LF ab dem ausstehenden Umbruch) und zeichnet danach das letzte
    /// Zeichen der umbrochenen Zeile absolut neu (CUP in die letzte Spalte). Zu erkennen an diesem CUP – dann bleibt die
    /// Korrektur für diesen Frame aus (Mitschnitt mit Windows PowerShell: volle Eingabezeile in der vorletzten Zeile).
    pub fn chunk_begin<T>(&mut self, t: &Term<T>, bytes: &[u8]) {
        if self.armed.is_some() && cup_to_last_column(bytes, t.columns()) {
            self.armed = None;
        }
    }

    /// Nach einem Lesestück: `short` = die Pipe war leer (weniger als der Lesepuffer gelesen), das Stück endet also
    /// mit einem Schreibvorgang von conhost (Frame-Ende).
    pub fn chunk_end<T>(&mut self, p: &Processor, t: &Term<T>, short: bool) {
        if !self.enabled || self.probe.is_some() {
            return;
        }
        if short && self.scan == Scan::Ground && p.sync_bytes_count() == 0 {
            if let Some(at) = pending_wrap_at(t) {
                self.armed = Some(at);
            }
        }
    }

    /// Neuerer conhost (fragt DA1 ab) bzw. mitgelieferte ConPTY: Korrektur abschalten. Entschieden wird nach den
    /// ersten `STARTUP_PROBE` Bytes.
    fn probe_startup(&mut self, bytes: &[u8]) {
        let Some(head) = self.probe.as_mut() else { return };
        head.extend_from_slice(&bytes[..bytes.len().min(STARTUP_PROBE - head.len().min(STARTUP_PROBE))]);
        if head.windows(3).any(|w| w == b"\x1b[c") || head.windows(4).any(|w| w == b"\x1b[0c") {
            self.enabled = false;
            self.armed = None;
            self.probe = None;
        } else if head.len() >= STARTUP_PROBE {
            self.probe = None;
        }
    }

    fn step(&mut self, b: u8) -> Effect {
        match self.scan {
            Scan::Ground => match b {
                0x1b => {
                    self.scan = Scan::Esc;
                    Effect::Quiet
                }
                b'\r' => Effect::Cr,
                0x07 => Effect::Quiet,
                _ => Effect::Other,
            },
            Scan::Esc => self.after_esc(b),
            Scan::Csi => match b {
                0x20..=0x3f => {
                    if self.params.len() < 16 {
                        self.params.push(b);
                    }
                    Effect::Quiet
                }
                0x40..=0x7e => {
                    self.scan = Scan::Ground;
                    let quiet = match b {
                        b'm' => true,
                        b'h' | b'l' => self.params.split(|&c| c == b';').all(|p| QUIET_MODES.contains(&p)),
                        _ => false,
                    };
                    if quiet {
                        Effect::Quiet
                    } else {
                        Effect::Other
                    }
                }
                0x1b => {
                    self.scan = Scan::Esc;
                    Effect::Other
                }
                0x18 | 0x1a => {
                    self.scan = Scan::Ground;
                    Effect::Other
                }
                // Steuerzeichen mitten in CSI führt vte aus (auch CR): nicht als Frame-Anfang werten.
                _ => Effect::Other,
            },
            Scan::Str => {
                match b {
                    0x07 | 0x18 | 0x1a => self.scan = Scan::Ground,
                    0x1b => self.scan = Scan::StrEsc,
                    _ => {}
                }
                Effect::Quiet
            }
            Scan::StrEsc => {
                if b == b'\\' {
                    self.scan = Scan::Ground;
                    Effect::Quiet
                } else {
                    self.after_esc(b)
                }
            }
        }
    }

    fn after_esc(&mut self, b: u8) -> Effect {
        match b {
            b'[' => {
                self.scan = Scan::Csi;
                self.params.clear();
                Effect::Quiet
            }
            b']' | b'P' | b'X' | b'^' | b'_' => {
                self.scan = Scan::Str;
                Effect::Quiet
            }
            0x1b => {
                self.scan = Scan::Esc;
                Effect::Other
            }
            _ => {
                self.scan = Scan::Ground;
                Effect::Other
            }
        }
    }
}

/// Enthält `bytes` einen Sprung in die letzte Spalte (`ESC [ Zeile ; Spalten H`)?
fn cup_to_last_column(bytes: &[u8], cols: usize) -> bool {
    let tail = format!(";{cols}H");
    let tail = tail.as_bytes();
    let mut i = 0;
    while let Some(off) = bytes[i..].windows(2).position(|w| w == b"\x1b[") {
        let start = i + off + 2;
        let digits = bytes[start..].iter().take_while(|b| b.is_ascii_digit()).count();
        if bytes[start + digits..].starts_with(tail) {
            return true;
        }
        i = start;
    }
    false
}

/// Cursor mit ausstehendem Umbruch (automatischer Umbruch an), aber nicht in der untersten Zeile.
fn pending_wrap_at<T>(t: &Term<T>) -> Option<Point> {
    let c = &t.grid().cursor;
    let wraps = c.input_needs_wrap && t.mode().contains(TermMode::LINE_WRAP);
    (wraps && c.point.line.0 + 1 < t.screen_lines() as i32).then_some(c.point)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::{test::TermSize, Config};

    struct Emu {
        term: Term<VoidListener>,
        p: Processor,
        cw: ConhostWrap,
    }

    impl Emu {
        fn new(cols: usize, rows: usize, system: bool) -> Self {
            let mut e = Emu { term: Term::new(Config::default(), &TermSize::new(cols, rows), VoidListener), p: Processor::new(), cw: ConhostWrap::new(system) };
            // Startfolge des alten conhost (ohne DA1)
            e.chunk("\x1b[6n");
            e.chunk("\x1b[?9001h\x1b[?1004h");
            e.chunk("\x1b[m\x1b]0;C:\\Program Files\\Git\\bin\\bash.exe\x07\x1b[?25h");
            e
        }

        /// Ein Lesestück (= ein Frame von conhost).
        fn chunk(&mut self, s: &str) {
            let Emu { term, p, cw } = self;
            cw.chunk_begin(term, s.as_bytes());
            cw.advance(p, term, s.as_bytes(), |p, t, b| p.advance(t, b), |t, w| w(t));
            cw.chunk_end(p, term, true);
        }

        /// Zeilen des Bildschirms (rechts ohne Leerzeichen), Cursor 1-basiert wie CPR.
        fn screen(&self) -> (Vec<String>, (usize, usize)) {
            let g = self.term.grid();
            let rows = (0..self.term.screen_lines())
                .map(|l| {
                    let row = &g[alacritty_terminal::index::Line(l as i32)];
                    (0..self.term.columns()).map(|c| row[alacritty_terminal::index::Column(c)].c).collect::<String>().trim_end().to_string()
                })
                .collect();
            let c = g.cursor.point;
            (rows, (c.line.0 as usize + 1, c.column.0 + 1))
        }
    }

    const W: &str = "wwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwww"; // 40

    /// Mitschnitte vom System-conhost (Windows 11 26200, Git Bash, 40×10 bzw. 40×5): Frames (Stücke) und die
    /// Cursorposition, die conhost danach per CPR meldet.
    #[test]
    fn replays_of_the_system_conhost_end_where_conhost_is() {
        // (Name, Zeilen, Frames, Cursor laut conhost nach dem letzten Zeichen)
        let cases: &[(&str, usize, &[&str], (usize, usize))] = &[
            // W, Pause, `\r\n\r\n`, Pause, Y
            ("s3", 10, &["a\r\nb\r\n", W, "\r\nY"], (5, 2)),
            // W, Pause, `\r\n`, Pause, `\r\n`, Pause, Y
            ("s10", 10, &["a\r\nb\r\n", W, "\r\nY"], (5, 2)),
            // dazwischen ein Frame nur mit einer Farbe
            ("p5", 10, &["a\r\nb\r\n", W, "\x1b[31m", "\r\nY\x1b[m\x1b[31m"], (5, 2)),
            // Drei Zeilenvorschübe bis in die unterste Zeile: der zweite rollt
            ("p3", 5, &["1\r\n2\r\n", W, "\r\n\nY"], (5, 2)),
            // ohne Pause: ein Frame, normaler Umbruch
            ("s7", 10, &["a\r\nb\r\n", &format!("{W}\r\nY")], (4, 2)),
            // ein einzelner Zeilenvorschub kommt gar nicht erst an, Y bricht selbst um
            ("s2", 10, &["a\r\nb\r\n", W, "Y"], (4, 2)),
            // CR zurück in dieselbe Zeile: conhost springt absolut
            ("s1", 10, &["a\r\nb\r\n", W, "\x1b[3;1HX"], (3, 2)),
            // CR in einem eigenen Frame, LF im nächsten
            ("s9", 10, &["a\r\nb\r\n", W, "\x1b[3;1H", "\r\nY"], (4, 2)),
            // unterste Zeile: conhost rollt mit CR LF und zeichnet das letzte Zeichen neu
            ("s11", 5, &["6\r\n7\r\n8\r\n9\r\n10\r\n", W, "\r\n\x1b[4;40Hw\r\nY"], (5, 2)),
            ("s5", 5, &["6\r\n7\r\n8\r\n9\r\n10\r\n", W, "\r\n\x1b[4;40Hw\r\n\nY"], (5, 2)),
        ];
        for (name, rows, frames, pos) in cases {
            let mut e = Emu::new(40, *rows, true);
            for f in *frames {
                e.chunk(f);
            }
            assert_eq!(e.screen().1, *pos, "{name}: Cursor wie conhost");
        }
    }

    #[test]
    fn the_blank_line_before_the_git_bash_prompt_is_kept() {
        // #264 (Mitschnitt #231, 100 Spalten): `printf 'w%.0s' $(seq 1 100); echo`, dann der Prompt mit der Leerzeile
        // aus PS1; danach springt conhost für das Echo des nächsten Befehls absolut in die Prompt-Zeile.
        let line = "w".repeat(100);
        let frames = [
            "\x1b[32m\r\nu@pc \x1b[35mMINGW64 \x1b[33m~\x1b[m\r\n$ ".to_string(),
            "\x1b[7mprintf 'w%.0s' $(seq 1 100); echo".to_string(),
            "\x1b[?2004l".to_string(),
            "\x1b[?25l\x1b[27m\x1b[3;3Hprintf 'w%.0s' $(seq 1 100); echo\r\n\x1b[?25h\x1b]133;C\x07".to_string(),
            line.clone(),
            "\x1b]133;D;0\x07".to_string(),
            "\x1b]9;9;\"C:\\Users\\u\"\x07".to_string(),
            "\x1b]133;A\x07".to_string(),
            "\x1b[?2004h".to_string(),
            "\x1b[32m\r\nu@pc \x1b[35mMINGW64 \x1b[33m~\x1b[m\r\n$ ".to_string(),
            "\x1b[7mecho next".to_string(),
            "\x1b[?2004l".to_string(),
            // conhost: Prompt in Zeile 7 (Leerzeile 5, Kopf 6)
            "\x1b[?25l\x1b[27m\x1b[7;3Hecho next\r\n\x1b[?25h\x1b]133;C\x07".to_string(),
            "next\r\n".to_string(),
        ];
        for (system, ok) in [(true, true), (false, false)] {
            let mut e = Emu::new(100, 30, system);
            for f in &frames {
                e.chunk(f);
            }
            let (rows, _) = e.screen();
            let expect = ["", "u@pc MINGW64 ~", "$ printf 'w%.0s' $(seq 1 100); echo", &line, "", "u@pc MINGW64 ~", "$ echo next", "next"];
            assert_eq!(rows[..8] == expect, ok, "System-ConPTY {system}: {rows:?}");
        }
    }

    #[test]
    fn a_newer_conhost_or_the_bundled_conpty_is_left_alone() {
        // OpenConsole 1.25: fragt beim Start DA1 ab und schickt den Zeilenvorschub selbst.
        let mut e = Emu { term: Term::new(Config::default(), &TermSize::new(40, 10), VoidListener), p: Processor::new(), cw: ConhostWrap::new(true) };
        e.chunk("\x1b[1t");
        e.chunk("\x1b[6n\x1b[c\x1b[?1004h\x1b[?9001h");
        assert!(!e.cw.enabled());
        e.chunk("a\r\nb\r\n");
        e.chunk(W);
        e.chunk("\r\n");
        e.chunk("Y");
        assert_eq!(e.screen().1, (4, 2));
        // Nicht Windows-ConPTY: nie aktiv.
        assert!(!ConhostWrap::new(false).enabled());
    }

    #[test]
    fn only_a_cr_right_after_the_frame_counts() {
        // Text, Cursorbewegung oder ein Lesestück mit vollem Puffer (Frame geht weiter) heben die Bereitschaft auf.
        for frames in [&["a\r\n", W, "\x1b[2;40H\r\nY"][..], &["a\r\n", W, "\x08\r\nY"][..], &["a\r\n", W, "\x1b[?1049h\x1b[?1049l\r\nY"][..]] {
            let mut e = Emu::new(40, 10, true);
            for f in frames {
                e.chunk(f);
            }
            assert_eq!(e.screen().1, (3, 2), "{frames:?}");
        }
        let mut e = Emu::new(40, 10, true);
        e.chunk("a\r\n");
        let Emu { term, p, cw } = &mut e;
        cw.advance(p, term, W.as_bytes(), |p, t, b| p.advance(t, b), |t, w| w(t));
        cw.chunk_end(p, term, false);
        e.chunk("\r\nY");
        assert_eq!(e.screen().1, (3, 2), "voller Lesepuffer: kein Frame-Ende");
        // OSC zwischen Frame-Ende und CR (Shell-Integration) ändert nichts.
        let mut e = Emu::new(40, 10, true);
        e.chunk("a\r\n");
        e.chunk(W);
        e.chunk("\x1b]133;D;0\x07\x1b]133;A\x1b\\");
        e.chunk("\r\nY");
        assert_eq!(e.screen().1, (4, 2));
    }

    #[test]
    fn a_frame_that_scrolls_below_the_full_line_is_left_alone() {
        // Mitschnitt (Windows PowerShell, 100×30, eingefügter Befehl über genau drei Zeilen): Die Eingabezeile endet in
        // der vorletzten Zeile mit ausstehendem Umbruch (Frame-Ende). Der nächste Frame rollt (CR LF, LF) und zeichnet
        // das letzte Zeichen der Zeile absolut neu – hier rechnet conhost wie xterm. Ein eingeschobener Umbruch rollte
        // eine Zeile zu viel, das `g` stand allein in einer Zeile, und `run_command` lieferte es als Ausgabe (#264).
        for system in [true, false] {
            let mut e = Emu::new(40, 5, system);
            e.chunk("1\r\n2\r\n3\r\n");
            e.chunk(&format!("\x1b[93m{}g\x1b[?25h", &W[1..]));
            e.chunk("\x1b[m\r\n\n\x1b[93m\x1b[3;40Hg\x1b[m\x1b[5;1H");
            e.chunk("Y");
            let (rows, cursor) = e.screen();
            let full = format!("{}g", &W[1..]);
            assert_eq!(rows, ["2", "3", full.as_str(), "", "Y"], "System-ConPTY {system}");
            assert_eq!(cursor, (5, 2));
        }
        // Ohne den Sprung in die letzte Spalte bleibt es beim Umbruch wie conhost (s3).
        let mut e = Emu::new(40, 5, true);
        e.chunk("1\r\n2\r\n");
        e.chunk(W);
        e.chunk("\r\nY");
        assert_eq!(e.screen().1, (5, 2));
        assert!(cup_to_last_column(b"\x1b[m\x1b[28;100Hg", 100) && !cup_to_last_column(b"\x1b[28;10Hg\x1b[100C", 100));
    }

    /// Ende eines Befehls mit Shell-Integration und der Prompt mit Leerzeile (Git Bash), Frame für Frame.
    const BASH_PROMPT: [&str; 5] = [
        "\x1b]133;D;0\x07",
        "\x1b]9;9;\"C:\\Users\\u\"\x07",
        "\x1b]133;A\x07",
        "\x1b[?2004h",
        "\x1b[32m\r\nu@pc \x1b[35mMINGW64 \x1b[33m~\x1b[m\r\n$ ",
    ];

    /// Getippter Text: conhost zeichnet jedes Zeichen in einem eigenen Frame.
    fn typed(e: &mut Emu, text: &str) {
        for ch in text.chars() {
            e.chunk(&ch.to_string());
        }
    }

    /// Mitschnitt (`conhost_capture`, Windows 11 26200, Git Bash, 60×30, Shell-Integration): Nach
    /// `printf 'x%.0s' $(seq 1 $COLUMNS); sleep 1; echo` (die Zeile endet allein in einem Frame, der Zeilenvorschub von
    /// `echo` kommt nie an) und Pausen tippt der Nutzer weitere Befehle. Der letzte ist länger als eine Zeile; nach Enter
    /// zeichnet conhost ihn absolut (CUP) in seiner Zeile neu. Ohne Korrektur stand das Terminal da schon eine Zeile zu
    /// hoch, das Echo erschien doppelt (#264). conhost meldet den Cursor danach in Zeile 16 (`POS 16 1`).
    #[test]
    fn the_echo_of_the_next_command_is_drawn_once() {
        let x = "x".repeat(60);
        let query = "IFS='[;' read -rs -d R -p $'\\e[6n' _ r c; echo \"POS $r $c\"";
        for system in [true, false] {
            let mut e = Emu::new(60, 30, system);
            e.chunk("\x1b[2J");
            e.chunk("\x1b[3J");
            e.chunk(&format!("\x1b[?25l{}\x1b[K\x1b[H\x1b[?25h", "\x1b[K\r\n".repeat(29)));
            let prompt = |e: &mut Emu| BASH_PROMPT.iter().for_each(|f| e.chunk(f));
            prompt(&mut e);
            typed(&mut e, "printf 'x%.0s' $(seq 1 $COLUMNS); sleep 1; echo");
            e.chunk("\x1b[?2004l");
            e.chunk("\r\n\x1b]133;C\x07");
            e.chunk(&x);
            prompt(&mut e);
            for cmd in ["echo next", "echo third"] {
                typed(&mut e, cmd);
                for f in ["\r\n", "\x1b[?2004l", "\x1b]133;C\x07", &format!("{}\r\n", &cmd[5..])] {
                    e.chunk(f);
                }
                prompt(&mut e);
            }
            // 58 Zeichen passen hinter „$ “, dann bricht readline mit Leerzeichen + CR um.
            typed(&mut e, &query[..57]);
            e.chunk(&format!("{} \r", &query[57..58]));
            typed(&mut e, &query[58..]);
            // Enter: conhost zeichnet die Eingabezeile absolut neu.
            e.chunk("\x1b[?2004l");
            e.chunk(&format!("\x1b[?25l\x1b[15;1H$ {query}\r\n\x1b[?25h\x1b]133;C\x07"));
            e.chunk("POS 16 1\r\n");
            prompt(&mut e);
            let (rows, cursor) = e.screen();
            let echoes = rows.iter().filter(|r| r.starts_with("$ IFS=")).count();
            if system {
                let expect = [
                    "",
                    "u@pc MINGW64 ~",
                    "$ printf 'x%.0s' $(seq 1 $COLUMNS); sleep 1; echo",
                    &x,
                    "",
                    "u@pc MINGW64 ~",
                    "$ echo next",
                    "next",
                    "",
                    "u@pc MINGW64 ~",
                    "$ echo third",
                    "third",
                    "",
                    "u@pc MINGW64 ~",
                    &format!("$ {}", &query[..58]),
                    "POS 16 1",
                    "",
                    "u@pc MINGW64 ~",
                    "$",
                ];
                assert_eq!(rows[..19], expect, "System-ConPTY: wie conhost");
                assert_eq!(cursor, (19, 3));
                assert_eq!(echoes, 1, "Echo genau einmal: {rows:?}");
            } else {
                assert_eq!(echoes, 2, "ohne Korrektur doppelt (Mitschnitt stimmt): {rows:?}");
            }
        }
    }

    /// Mitschnitt erneut abspielen (von Hand): `CAP_REPLAY=<datei.json>` (Liste der Lesestücke als JSON-Zeichenketten),
    /// `CAP_COLS`/`CAP_ROWS`; gibt aus, nach welchem Stück sich die Cursorzeilen mit/ohne Korrektur trennen.
    #[test]
    #[ignore]
    fn conhost_replay() {
        let path = std::env::var("CAP_REPLAY").expect("CAP_REPLAY");
        let cols: usize = std::env::var("CAP_COLS").ok().and_then(|v| v.parse().ok()).unwrap_or(100);
        let rows: usize = std::env::var("CAP_ROWS").ok().and_then(|v| v.parse().ok()).unwrap_or(30);
        let chunks: Vec<String> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let mut with = Emu { term: Term::new(Config::default(), &TermSize::new(cols, rows), VoidListener), p: Processor::new(), cw: ConhostWrap::new(true) };
        let mut without = Emu { term: Term::new(Config::default(), &TermSize::new(cols, rows), VoidListener), p: Processor::new(), cw: ConhostWrap::new(false) };
        let (mut diff, mut same) = (0i64, true);
        for (i, c) in chunks.iter().enumerate() {
            let before = (with.screen().1, without.screen().1);
            with.chunk(c);
            without.chunk(c);
            let d = with.screen().1 .0 as i64 - without.screen().1 .0 as i64;
            let s = with.screen().0 == without.screen().0;
            if d != diff || s != same {
                println!("Stück {i}: Abstand {diff} -> {d}, gleicher Bildschirm {same} -> {s}, Cursor vorher {before:?}, nachher {:?}/{:?}: {c:?}", with.screen().1, without.screen().1);
                (diff, same) = (d, s);
            }
        }
        for (name, e) in [("mit Korrektur", &with), ("ohne Korrektur", &without)] {
            let (screen, cur) = e.screen();
            println!("--- {name}: Cursor {cur:?} ---");
            for (i, l) in screen.iter().enumerate() {
                println!("{:>2}|{l}", i + 1);
            }
        }
    }

    /// Mitschnitt vom System-conhost (#264), von Hand gestartet (Git Bash unter `C:\Program Files\Git`):
    /// `cargo test --release conhost_capture -- --ignored --nocapture`. Startet Git Bash ohne Profil (eigenes HOME im
    /// Temp-Ordner, ohne Verlaufsdatei) mit der Shell-Integration auf der ConPTY von Windows, tippt die Schritte der
    /// Reproduktion und gibt die Lesestücke als Rust-Zeichenketten aus, dazu den Bildschirm der Emulation mit und ohne
    /// Korrektur. Zum Schluss fragt die Shell conhost nach dem Cursor (`POS Zeile Spalte`): Er muss in der Zeile dieser
    /// Ausgabe stehen. Einstellbar: `CAP_COLS`, `CAP_ROWS`, `CAP_SHELL=powershell`, `CAP_HOME`, `CAP_READ_DELAY`,
    /// `CAP_STEPS` (`Text@@ms je Zeichen@@Pause;;…`, `<CR>`, `<PAD>`/`<PAD2>`/`<PAD3>` füllen Prompt + Befehl auf genau
    /// 1–3 Zeilen). Markiert im Mitschnitt, wo sich die Cursorzeilen mit/ohne Korrektur trennen.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn conhost_capture() {
        use portable_pty::{CommandBuilder, PtySize, PtySystem};
        use std::io::{Read, Write};
        use std::time::{Duration, Instant};
        let cols: usize = std::env::var("CAP_COLS").ok().and_then(|v| v.parse().ok()).unwrap_or(60);
        let rows: usize = std::env::var("CAP_ROWS").ok().and_then(|v| v.parse().ok()).unwrap_or(12);
        let pause: u64 = std::env::var("CAP_PAUSE").ok().and_then(|v| v.parse().ok()).unwrap_or(3000);
        let sys = portable_pty::win::conpty::ConPtySystem::with_sideloaded_conpty(None);
        let pair = sys.openpty(PtySize { rows: rows as u16, cols: cols as u16, pixel_width: 0, pixel_height: 0 }).unwrap();
        let git = r"C:\Program Files\Git";
        // CAP_HOME: eigener (leerer, neuer) Ordner, z. B. für eine bestimmte Prompt-Länge.
        let home = std::env::var("CAP_HOME").map(std::path::PathBuf::from).unwrap_or_else(|_| std::env::temp_dir().join(format!("pd-conhost-cap-{}", std::process::id())));
        std::fs::create_dir_all(&home).unwrap();
        // CAP_SHELL=powershell: Windows PowerShell ohne Profil (mit Shell-Integration), Prompt `PS <HOME>> `.
        let powershell = std::env::var("CAP_SHELL").is_ok_and(|s| s == "powershell");
        let bash = if powershell { "powershell.exe".to_string() } else { format!(r"{git}\bin\bash.exe") };
        let mut cmd = CommandBuilder::new(&bash);
        let spec = crate::shells::ShellSpec { id: "s".into(), label: "s".into(), program: bash.clone(), args: vec![] };
        let plan = crate::shells::integration(&spec, &std::env::temp_dir(), &crate::shellinit::EnvCtx { env: &[], inherit: true });
        if powershell {
            cmd.arg("-NoLogo");
            cmd.arg("-NoProfile");
            for a in &plan.args {
                cmd.arg(a);
            }
        } else {
            for a in ["--noprofile", "--norc", "-i"] {
                cmd.arg(a);
            }
        }
        // `<PAD>`/`<PAD2>` in einem Schritt: so viele `a`, dass Prompt + Text genau eine bzw. zwei Zeilen füllen.
        let prompt_len = if powershell { format!("PS {}> ", home.display()).chars().count() } else { 2 };
        cmd.cwd(&home);
        cmd.env("HOME", &home);
        cmd.env("HISTFILE", "");
        cmd.env("TERM", "xterm-256color");
        cmd.env("PATH", format!(r"{git}\usr\bin;{}", std::env::var("PATH").unwrap_or_default()));
        cmd.env("PS1", "\\n\\[\\033[32m\\]u@pc \\[\\033[35m\\]MINGW64 \\[\\033[33m\\]~\\[\\033[0m\\]\\n$ ");
        for (k, v) in &plan.env {
            cmd.env(k, v);
        }
        let mut child = pair.slave.spawn_command(cmd).unwrap();
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().unwrap();
        let writer = pair.master.take_writer().unwrap();
        let t0 = Instant::now();
        let (tx, rx) = std::sync::mpsc::channel::<(Duration, Vec<u8>)>();
        // CAP_READ_DELAY: Pause vor jedem Lesen (ms), damit Schreibvorgänge von conhost zu einem Lesestück verschmelzen.
        let read_delay: u64 = std::env::var("CAP_READ_DELAY").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 65536];
            loop {
                std::thread::sleep(Duration::from_millis(read_delay));
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send((t0.elapsed(), buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let with = Emu { term: Term::new(Config::default(), &TermSize::new(cols, rows), VoidListener), p: Processor::new(), cw: ConhostWrap::new(true) };
        let without = Emu { term: Term::new(Config::default(), &TermSize::new(cols, rows), VoidListener), p: Processor::new(), cw: ConhostWrap::new(false) };
        struct Cap {
            rx: std::sync::mpsc::Receiver<(Duration, Vec<u8>)>,
            writer: Box<dyn Write + Send>,
            log: Vec<(Duration, String)>,
            with: Emu,
            without: Emu,
            diff: i64,
        }
        impl Cap {
            fn pump(&mut self, ms: u64) {
                let end = Instant::now() + Duration::from_millis(ms);
                while let Some(left) = end.checked_duration_since(Instant::now()) {
                    let Ok((at, bytes)) = self.rx.recv_timeout(left) else { continue };
                    let s = String::from_utf8_lossy(&bytes).into_owned();
                    self.with.chunk(&s);
                    self.without.chunk(&s);
                    // Abstand der Cursorzeilen mit/ohne Korrektur geändert: im Mitschnitt markieren.
                    let d = self.with.screen().1 .0 as i64 - self.without.screen().1 .0 as i64;
                    if d != self.diff {
                        self.log.push((at, format!("<<Abstand {} -> {d} nach dem nächsten Stück>>", self.diff)));
                        self.diff = d;
                    }
                    if s.contains("[6n") {
                        let (_, (r, c)) = self.with.screen();
                        let _ = self.writer.write_all(format!("[{r};{c}R").as_bytes());
                    }
                    self.log.push((at, s));
                }
            }
            fn typ(&mut self, text: &str, per_char: u64) {
                if per_char == 0 {
                    // auf einmal samt Enter (wie `run_command`)
                    let _ = self.writer.write_all(text.as_bytes());
                    return;
                }
                for ch in text.chars() {
                    let _ = self.writer.write_all(ch.to_string().as_bytes());
                    let _ = self.writer.flush();
                    self.pump(per_char);
                }
            }
        }
        let mut cap = Cap { rx, writer, log: Vec::new(), with, without, diff: 0 };
        cap.pump(2000);
        let steps: Vec<(String, u64, u64)> = match std::env::var("CAP_STEPS") {
            // Schritte: Text@@ms je Zeichen (0 = auf einmal)@@Pause danach, getrennt durch `;;`, `\r` als `<CR>`
            Ok(s) => s
                .split(";;")
                .map(|st| {
                    let mut it = st.split("@@");
                    let mut text = it.next().unwrap_or("").replace("<CR>", "\r");
                    for (tag, k) in [("<PAD3>", 3), ("<PAD2>", 2), ("<PAD>", 1)] {
                        if text.contains(tag) {
                            let rest = text.replace(tag, "").replace('\r', "").chars().count();
                            text = text.replace(tag, &"a".repeat((cols * k).saturating_sub(prompt_len + rest)));
                        }
                    }
                    (text, it.next().and_then(|v| v.parse().ok()).unwrap_or(20), it.next().and_then(|v| v.parse().ok()).unwrap_or(1000))
                })
                .collect(),
            Err(_) => vec![
                ("printf '%*s\\n' \"$COLUMNS\" '' | tr ' ' x\r".into(), 15, pause),
                ("echo next\r".into(), 120, pause),
                ("echo third\r".into(), 15, 1500),
            ],
        };
        for (text, per, after) in &steps {
            cap.typ(text, *per);
            cap.pump(*after);
        }
        if powershell {
            cap.typ("$p = $Host.UI.RawUI.CursorPosition; \"POS $($p.Y + 1) $($p.X + 1)\"\r", 5);
        } else {
            cap.typ("IFS='[;' read -rs -d R -p $'\\e[6n' _ r c; echo \"POS $r $c\"\r", 5);
        }
        cap.pump(1500);
        let _ = cap.writer.write_all(b"exit\r");
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&home);
        println!("--- Lesestücke ({cols}x{rows}) ---");
        for (at, s) in &cap.log {
            println!("{:>6} {:?},", at.as_millis(), s);
        }
        for (name, e) in [("mit Korrektur", &cap.with), ("ohne Korrektur", &cap.without)] {
            let (screen, cur) = e.screen();
            println!("--- {name}: Cursor {cur:?} ---");
            for (i, l) in screen.iter().enumerate() {
                println!("{:>2}|{l}", i + 1);
            }
        }
    }
}
