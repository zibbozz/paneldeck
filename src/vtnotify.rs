//! Meldungen von Programmen im Terminal (#183): Fortschritt (OSC 9;4), Benachrichtigungen (OSC 9, OSC 777) und die
//! Glocke (BEL). Hier liegt nur die reine Logik (Auswerten, Bereinigen, Ratenbegrenzung); erkannt werden die Sequenzen
//! in `osc.rs`, angezeigt in der App (`poll_shell`, Seitenleiste, Taskleiste).
//!
//! * **Fortschritt** `ESC ] 9 ; 4 ; <st> ; <pr> ST` (ConEmu/Windows Terminal): `st` 0 = aus, 1 = normal (`pr` 0–100),
//!   2 = Fehler, 3 = unbestimmt, 4 = angehalten. Ein fehlender oder ungültiger Prozentwert gilt als 0 bzw. behält bei
//!   Fehler/angehalten den Wert 0 (wie Windows Terminal).
//! * **Benachrichtigung** `ESC ] 9 ; <text> ST` (iTerm2) – aber nicht die ConEmu-Befehle `9;<zahl>;…` – und
//!   `ESC ] 777 ; notify ; <titel> ; <text> ST` (urxvt/foot/Ghostty).
//! * Texte kommen von beliebigen Programmen: Steuerzeichen (C0/C1), Bidi-Steuerzeichen und unsichtbare Formatzeichen
//!   werden entfernt, Leerraum zusammengefasst, die Länge begrenzt ([`sanitize`]).

use std::time::{Duration, Instant};

/// Höchstlänge (Zeichen) des Titels einer Benachrichtigung.
pub const TITLE_MAX: usize = 64;
/// Höchstlänge (Zeichen) des Textes einer Benachrichtigung.
pub const BODY_MAX: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressState {
    Normal,
    Error,
    Indeterminate,
    Paused,
}

impl ProgressState {
    /// Name für die Agenten-Schnittstelle (`list_sessions`).
    pub fn as_str(self) -> &'static str {
        match self {
            ProgressState::Normal => "normal",
            ProgressState::Error => "error",
            ProgressState::Indeterminate => "indeterminate",
            ProgressState::Paused => "paused",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub state: ProgressState,
    /// 0–100 (bei `Indeterminate` ohne Bedeutung, dann 0).
    pub percent: u8,
}

/// Parameter hinter `9;4` auswerten (`rest` = alles nach `9;4`, z. B. `;1;50`). `None` = ungültig (ignorieren),
/// `Some(None)` = Fortschritt entfernen, `Some(Some(p))` = setzen.
pub fn parse_progress(rest: &str) -> Option<Option<Progress>> {
    let rest = match rest.strip_prefix(';') {
        Some(r) => r,
        None if rest.is_empty() => return Some(None),
        None => return None,
    };
    let mut it = rest.split(';');
    let st = it.next().unwrap_or("").trim();
    let st: u8 = if st.is_empty() { 0 } else { st.parse().ok()? };
    let pr = it.next().map(str::trim).unwrap_or("");
    let pct = if pr.is_empty() { 0 } else { pr.parse::<u32>().ok()?.min(100) as u8 };
    let state = match st {
        0 => return Some(None),
        1 => ProgressState::Normal,
        2 => ProgressState::Error,
        3 => return Some(Some(Progress { state: ProgressState::Indeterminate, percent: 0 })),
        4 => ProgressState::Paused,
        _ => return None,
    };
    Some(Some(Progress { state, percent: pct }))
}

/// Unbestimmter Fortschritt verfällt nach dieser Zeit ohne neue Meldung (Seitenleiste animiert ihn).
pub const INDETERMINATE_TTL: Duration = Duration::from_secs(10 * 60);

/// Ist der gemeldete Fortschritt verfallen? Nur unbestimmter, nach `INDETERMINATE_TTL` ohne Meldung.
pub fn progress_expired(p: Option<Progress>, at: Option<Instant>, now: Instant) -> bool {
    matches!(p, Some(Progress { state: ProgressState::Indeterminate, .. }))
        && at.is_some_and(|t| now.saturating_duration_since(t) >= INDETERMINATE_TTL)
}

/// Ist der Text hinter `9;` ein ConEmu-Befehl (`<zahl>` oder `<zahl>;…`) statt einer Benachrichtigung?
pub fn is_conemu_command(text: &str) -> bool {
    let num = text.split(';').next().unwrap_or("");
    !num.is_empty() && num.bytes().all(|b| b.is_ascii_digit())
}

/// Zeichen, die in angezeigten Texten aus dem Terminal nie vorkommen dürfen: Bidi-Steuerzeichen (Umkehrung von Text,
/// „Trojan Source“), unsichtbare Format- und Trennzeichen.
/// Gilt auch für Link-Ziele (`hyperlink`): weiches Trennzeichen, Graphem-Verbinder, mongolischer Vokaltrenner,
/// Variationsselektoren und Tag-Zeichen sind unsichtbar und taugen zum Verschleiern.
pub fn is_hidden_format(c: char) -> bool {
    matches!(c,
        '\u{00ad}' | '\u{034f}' | '\u{061c}' | '\u{180e}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}'
        | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{206a}'..='\u{206f}' | '\u{fe00}'..='\u{fe0f}'
        | '\u{feff}' | '\u{fff9}'..='\u{fffb}' | '\u{e0000}'..='\u{e007f}')
}

/// Text aus dem Terminal für die Anzeige bereinigen: Steuerzeichen und Bidi-/Formatzeichen weg, Leerraum (auch
/// Zeilenumbrüche) zu einem Leerzeichen, vorne/hinten gekürzt, höchstens `max` Zeichen (dann mit „…“).
pub fn sanitize(s: &str, max: usize) -> String {
    let mut out = String::with_capacity(s.len().min(max * 4));
    let mut n = 0usize;
    let mut space = false;
    for c in s.chars() {
        if c.is_whitespace() || c == '\u{2028}' || c == '\u{2029}' {
            space = !out.is_empty();
            continue;
        }
        if c.is_control() || is_hidden_format(c) {
            continue;
        }
        if space {
            if n + 1 >= max {
                out.push('…');
                return out;
            }
            out.push(' ');
            n += 1;
            space = false;
        }
        if n + 1 > max {
            out.pop();
            out.push('…');
            return out;
        }
        out.push(c);
        n += 1;
    }
    out
}

/// Ratenbegrenzung je Sitzung (Eimer mit Marken): höchstens `burst` Meldungen auf einmal, danach eine je `every`.
#[derive(Debug, Clone)]
pub struct RateLimit {
    tokens: f32,
    last: Instant,
}

/// Größe des Eimers.
pub const RATE_BURST: f32 = 3.0;
/// Eine neue Meldung je Zeitspanne.
pub const RATE_EVERY: Duration = Duration::from_secs(20);

impl RateLimit {
    pub fn new(now: Instant) -> Self {
        RateLimit { tokens: RATE_BURST, last: now }
    }

    /// Darf jetzt eine Meldung angezeigt werden? Verbraucht dann eine Marke.
    pub fn allow(&mut self, now: Instant) -> bool {
        let dt = now.saturating_duration_since(self.last).as_secs_f32();
        self.last = now;
        self.tokens = (self.tokens + dt / RATE_EVERY.as_secs_f32()).min(RATE_BURST);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_parsing() {
        let p = |st, percent| Some(Some(Progress { state: st, percent }));
        assert_eq!(parse_progress(";1;50"), p(ProgressState::Normal, 50));
        assert_eq!(parse_progress(";1;250"), p(ProgressState::Normal, 100));
        assert_eq!(parse_progress(";1"), p(ProgressState::Normal, 0));
        assert_eq!(parse_progress(";2;30"), p(ProgressState::Error, 30));
        assert_eq!(parse_progress(";3"), p(ProgressState::Indeterminate, 0));
        assert_eq!(parse_progress(";3;77"), p(ProgressState::Indeterminate, 0));
        assert_eq!(parse_progress(";4;10"), p(ProgressState::Paused, 10));
        assert_eq!(parse_progress(";0"), Some(None));
        assert_eq!(parse_progress(";0;50"), Some(None));
        assert_eq!(parse_progress(""), Some(None));
        assert_eq!(parse_progress(";"), Some(None));
        for bad in [";5;1", ";x", ";1;-1", ";1;abc", "x", ";99999999999"] {
            assert_eq!(parse_progress(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn indeterminate_progress_expires() {
        let t0 = Instant::now();
        let ind = Some(Progress { state: ProgressState::Indeterminate, percent: 0 });
        let norm = Some(Progress { state: ProgressState::Normal, percent: 5 });
        assert!(!progress_expired(ind, Some(t0), t0 + Duration::from_secs(60)));
        assert!(progress_expired(ind, Some(t0), t0 + INDETERMINATE_TTL));
        assert!(!progress_expired(norm, Some(t0), t0 + INDETERMINATE_TTL * 10));
        assert!(!progress_expired(None, Some(t0), t0 + INDETERMINATE_TTL));
    }

    #[test]
    fn conemu_commands_are_not_notifications() {
        for cmd in ["4;1;50", "9;\"C:\\\"", "1;500", "12", "7;calc.exe"] {
            assert!(is_conemu_command(cmd), "{cmd:?}");
        }
        for text in ["Build done", "4 tests failed", "", "done;ok", "1a;x"] {
            assert!(!is_conemu_command(text), "{text:?}");
        }
    }

    #[test]
    fn sanitizing() {
        assert_eq!(sanitize("  Build\n\tfertig  ", 64), "Build fertig");
        assert_eq!(sanitize("a\x07b\x1b[31mc\u{7f}d\u{9b}e", 64), "ab[31mcde");
        assert_eq!(sanitize("evil\u{202e}txt.exe\u{202c}\u{2066}x\u{2069}\u{200f}\u{feff}", 64), "eviltxt.exex");
        assert_eq!(sanitize("a\u{2028}b", 64), "a b");
        assert_eq!(sanitize("\u{200b}\u{200b}", 64), "");
        // Länge: höchstens `max` Zeichen einschließlich „…“.
        let long = sanitize(&"x".repeat(1000), 10);
        assert_eq!(long.chars().count(), 10);
        assert!(long.ends_with('…'));
        assert_eq!(sanitize(&"ä".repeat(10), 10), "ä".repeat(10));
        assert_eq!(sanitize("abc def", 4), "abc…");
        assert_eq!(sanitize("Emoji 👍🏽 ok", 64), "Emoji 👍🏽 ok");
        assert_eq!(sanitize("so\u{00ad}ft\u{034f}\u{180e}\u{fe0f}\u{e0041}x", 64), "softx");
    }

    #[test]
    fn rate_limit() {
        let t0 = Instant::now();
        let mut r = RateLimit::new(t0);
        assert!(r.allow(t0) && r.allow(t0) && r.allow(t0));
        assert!(!r.allow(t0));
        assert!(!r.allow(t0 + Duration::from_secs(10)));
        assert!(r.allow(t0 + Duration::from_secs(21)));
        assert!(!r.allow(t0 + Duration::from_secs(22)));
        // Nach langer Pause wieder ein voller Eimer, aber nicht mehr.
        let t1 = t0 + Duration::from_secs(3600);
        assert!(r.allow(t1) && r.allow(t1) && r.allow(t1));
        assert!(!r.allow(t1));
    }
}
