//! Erkennung von Shell-Integrations-Sequenzen (OSC) im Ausgabestrom:
//!  - `ESC ] 133 ; A/B/D[;exit] BEL`  Prompt-Anfang/-Ende, Befehl beendet (FinalTerm/VS-Code/WezTerm-Konvention)
//!  - `ESC ] 9 ; 9 ; "pfad" BEL`      aktueller Ordner (ConEmu/Windows Terminal)
//!  - `ESC ] 7 ; file://host/pfad BEL` aktueller Ordner (OSC 7)
//!  - `ESC ] 7777 ; dur=<ms> BEL`      Dauer des letzten Befehls (eigene Erweiterung)
//! Die Bytes werden zusätzlich unverändert an die Terminal-Emulation weitergegeben (unbekannte OSC werden dort ignoriert).

#[derive(Debug, Clone, PartialEq)]
pub enum OscEvent {
    PromptStart,
    CommandStart,
    CommandDone { exit: Option<i32> },
    Cwd(String),
    DurationMs(u64),
}

#[derive(Default)]
enum State {
    #[default]
    Normal,
    Esc,
    Osc(Vec<u8>),
    OscEsc(Vec<u8>),
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
                State::Esc => match b {
                    b']' => State::Osc(Vec::new()),
                    0x1b => State::Esc,
                    _ => State::Normal,
                },
                State::Osc(mut buf) => match b {
                    0x07 => {
                        parse(&buf, &mut out);
                        State::Normal
                    }
                    0x1b => State::OscEsc(buf),
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
                    b']' => State::Osc(Vec::new()),
                    _ => State::Normal,
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn ignores_garbage_and_other_osc() {
        let mut s = OscScanner::default();
        assert!(s.feed(b"\x1b[31mfarbig\x1b[0m \x1b]0;Titel\x07 \x1b]133;B\x07").is_empty());
    }
}
