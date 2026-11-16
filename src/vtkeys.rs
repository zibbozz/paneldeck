//! Tasten → Bytes fürs Terminal (#183, Teil 2b). Drei Kodierungen, abhängig vom Modus der Sitzung:
//!
//! - **kitty-Tastaturprotokoll** (CSI u), sobald ein Programm es mit `CSI > flags u` einschaltet (alacritty führt
//!   den Stapel, beantwortet `CSI ? u`). Hat Vorrang: Nur Programme mit VT-Eingabe schalten es ein, und beide
//!   ConPTY-Varianten (System-conhost und gebündeltes OpenConsole) reichen `CSI 13;2u` unverändert an sie durch.
//! - **win32-input-mode** (DECSET 9001): ConPTY verlangt ihn beim Start selbst (`CSI ?9001h`) und übersetzt
//!   `CSI Vk;Sc;Uc;Kd;Cs;Rc _` in echte Tastenereignisse. Klassische Konsolenprogramme (PSReadLine, cmd) sehen dann
//!   z. B. Strg+Leertaste statt Strg+Umschalt+2. Programme mit VT-Eingabe (Claude Code, WSL, Git Bash, node) bekommen
//!   aber nur die Übersetzung von conhost – und die macht aus Umschalt+Enter ein nacktes `\r`. Deshalb wird
//!   win32-input NUR für Tasten benutzt, bei denen die Sondierung gezeigt hat, dass beide Programmarten mindestens
//!   so gut fahren wie mit der klassischen Kodierung (`win32_key`). Umschalt+Enter bleibt `ESC CR`.
//! - **klassisch** (xterm/VT) für alles andere.
//!
//! Text (Buchstaben, AltGr-Zeichen, Tottasten, IME) kommt als `egui::Event::Text` und läuft über `encode_text`;
//! nur reine Alt-Kombinationen und (mit kitty-Flag 8) alle Tasten werden aus dem Tastenereignis kodiert.

use alacritty_terminal::term::TermMode;
use eframe::egui::{Key, Modifiers};

/// kitty: Escape-Codes eindeutig machen (Esc, Strg/Alt+Zeichen, Enter/Tab/Rück mit Modifikatoren als CSI u).
pub const KITTY_DISAMBIGUATE: u8 = 1;
/// kitty: Wiederholen und Loslassen melden.
pub const KITTY_EVENT_TYPES: u8 = 2;
/// kitty: umgeschaltete Taste und Taste der Grundbelegung (US) mitsenden.
pub const KITTY_ALTERNATE_KEYS: u8 = 4;
/// kitty: alle Tasten als Escape-Code, auch Text ohne Modifikatoren.
pub const KITTY_ALL_AS_ESC: u8 = 8;
/// kitty: erzeugten Text mitsenden (nur zusammen mit Flag 8).
pub const KITTY_TEXT: u8 = 16;

/// Was die Kodierung aus dem Zustand der Emulation braucht.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyMode {
    /// DECCKM: Pfeile als `ESC O A`.
    pub app_cursor: bool,
    /// DECKPAM: Ziffernblock als `ESC O p` … (nur ohne win32-input; dort übersetzt conhost selbst).
    pub app_keypad: bool,
    /// DECSET 9001 (win32-input-mode) ist aktiv – unter Windows verlangt ConPTY ihn beim Start.
    pub win32: bool,
    /// Aktive kitty-Flags (Bits `KITTY_*`), 0 = aus.
    pub kitty: u8,
}

impl KeyMode {
    pub fn new(mode: TermMode, win32: bool) -> KeyMode {
        let mut kitty = 0u8;
        for (flag, bit) in [
            (TermMode::DISAMBIGUATE_ESC_CODES, KITTY_DISAMBIGUATE),
            (TermMode::REPORT_EVENT_TYPES, KITTY_EVENT_TYPES),
            (TermMode::REPORT_ALTERNATE_KEYS, KITTY_ALTERNATE_KEYS),
            (TermMode::REPORT_ALL_KEYS_AS_ESC, KITTY_ALL_AS_ESC),
            (TermMode::REPORT_ASSOCIATED_TEXT, KITTY_TEXT),
        ] {
            if mode.contains(flag) {
                kitty |= bit;
            }
        }
        KeyMode { app_cursor: mode.contains(TermMode::APP_CURSOR), app_keypad: mode.contains(TermMode::APP_KEYPAD), win32, kitty }
    }

    /// kitty kodiert (Flag 1 oder 8). Die Flags 2/4/16 allein ändern nichts an den Bytes.
    fn kitty_on(&self) -> bool {
        self.kitty & (KITTY_DISAMBIGUATE | KITTY_ALL_AS_ESC) != 0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Press,
    Repeat,
    Release,
}

/// Ein Tastenereignis (aus `egui::Event::Key`).
#[derive(Clone, Copy, Debug)]
pub struct KeyPress<'a> {
    /// Logische Taste (Tastaturbelegung).
    pub key: Key,
    /// Position auf einer US-Tastatur (`egui::Event::Key::physical_key`).
    pub physical: Option<Key>,
    pub mods: Modifiers,
    pub phase: Phase,
    /// Taste des Ziffernblocks (bei eingeschaltetem NumLock).
    pub numpad: bool,
    /// Text, den die Taste erzeugt (das direkt folgende `egui::Event::Text`).
    pub text: Option<&'a str>,
}

impl<'a> KeyPress<'a> {
    pub fn new(key: Key, mods: Modifiers) -> Self {
        KeyPress { key, physical: None, mods, phase: Phase::Press, numpad: false, text: None }
    }
}

/// Ergebnis von `encode_key`: Bytes (leer = nichts senden) und ob das folgende `Event::Text` schon erledigt ist.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Encoded {
    pub bytes: Vec<u8>,
    pub eat_text: bool,
}

impl Encoded {
    fn bytes(b: impl Into<Vec<u8>>) -> Self {
        Encoded { bytes: b.into(), eat_text: false }
    }
    fn with_text(b: impl Into<Vec<u8>>) -> Self {
        Encoded { bytes: b.into(), eat_text: true }
    }
}

/// AltGr kommt unter Windows als Strg+Alt; solche Zeichen laufen über den Text.
fn altgr(m: Modifiers) -> bool {
    m.ctrl && m.alt
}

/// Modifikator-Parameter (xterm und kitty): 1 + Umschalt 1 + Alt 2 + Strg 4.
fn mod_param(m: Modifiers) -> u8 {
    1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.ctrl as u8
}

/// Zeichen einer Taste ohne Umschalt (Buchstaben klein). `None` für Tasten ohne Zeichen.
fn key_char(key: Key) -> Option<char> {
    use Key::*;
    Some(match key {
        Space => ' ',
        Colon => ':',
        Comma => ',',
        Backslash => '\\',
        Slash => '/',
        Pipe => '|',
        Questionmark => '?',
        Exclamationmark => '!',
        OpenBracket => '[',
        CloseBracket => ']',
        OpenCurlyBracket => '{',
        CloseCurlyBracket => '}',
        Backtick => '`',
        Minus => '-',
        Period => '.',
        Plus => '+',
        Equals => '=',
        Semicolon => ';',
        Quote => '\'',
        Num0 => '0',
        Num1 => '1',
        Num2 => '2',
        Num3 => '3',
        Num4 => '4',
        Num5 => '5',
        Num6 => '6',
        Num7 => '7',
        Num8 => '8',
        Num9 => '9',
        _ => {
            let n = key.name();
            let b = n.as_bytes();
            if b.len() == 1 && b[0].is_ascii_uppercase() {
                (b[0] as char).to_ascii_lowercase()
            } else {
                return None;
            }
        }
    })
}

fn is_letter(key: Key) -> bool {
    key_char(key).is_some_and(|c| c.is_ascii_lowercase())
}

/// Ein einzelnes druckbares Zeichen aus dem Text des Ereignisses.
fn single_char(text: Option<&str>) -> Option<char> {
    let t = text?;
    let mut it = t.chars();
    let c = it.next()?;
    (it.next().is_none() && !c.is_control()).then_some(c)
}

/// Zeichen, das die Taste mit den aktuellen Modifikatoren tippt (für Alt+Taste): bevorzugt der Text des Ereignisses.
fn typed_char(k: &KeyPress) -> Option<char> {
    if let Some(c) = single_char(k.text) {
        return Some(c);
    }
    let c = key_char(k.key)?;
    Some(if k.mods.shift && c.is_ascii_lowercase() { c.to_ascii_uppercase() } else { c })
}

/// Kodiert ein Tastenereignis. `eat_text`: das folgende `Event::Text` gehört zu dieser Taste und ist schon gesendet.
pub fn encode_key(k: &KeyPress, mode: KeyMode) -> Encoded {
    if mode.kitty_on() {
        return kitty_key(k, mode);
    }
    // Ohne kitty gibt es kein Loslassen; Wiederholen = erneutes Drücken.
    if k.phase == Phase::Release {
        return Encoded::default();
    }
    if k.numpad && !k.mods.ctrl && !k.mods.alt {
        if let Some(c) = single_char(k.text).or_else(|| key_char(k.key)) {
            if let Some(b) = numpad(c, k.mods, mode) {
                return Encoded::with_text(b);
            }
        }
    }
    if mode.win32 {
        if let Some(b) = win32_key(k) {
            return Encoded::with_text(b);
        }
    }
    legacy_key(k, mode)
}

/// Kodiert Text, der nicht schon mit seiner Taste gesendet wurde (`Event::Text`). `numpad`: kam vom Ziffernblock.
pub fn encode_text(text: &str, mods: Modifiers, numpad_key: bool, mode: KeyMode) -> Vec<u8> {
    if text.chars().all(|c| c.is_control()) {
        return Vec::new();
    }
    let pure_ctrl = mods.ctrl && !mods.alt;
    let pure_alt = mods.alt && !mods.ctrl;
    if pure_ctrl {
        return Vec::new(); // Steuerzeichen kommen aus dem Tastenereignis
    }
    if pure_alt {
        // Alt+Zeichen ohne eigenes Tastenereignis (z. B. Alt+ö): wie Alt+Taste.
        if mode.kitty_on() {
            if let Some(c) = single_char(Some(text)) {
                let base = c.to_lowercase().next().unwrap_or(c);
                let m = Modifiers { shift: mods.shift || base != c, ..mods };
                let shifted = (base != c).then_some(c as u32);
                return csi_u(base as u32, shifted, None, mod_param(m), Phase::Press, None, mode);
            }
        }
        let mut v = vec![0x1b];
        v.extend_from_slice(text.as_bytes());
        return v;
    }
    if numpad_key && !altgr(mods) {
        if let Some(c) = single_char(Some(text)) {
            if let Some(b) = numpad(c, mods, mode) {
                return b;
            }
        }
    }
    text.as_bytes().to_vec()
}

// ───────────────────────────── klassisch (xterm) ─────────────────────────────

fn legacy_key(k: &KeyPress, mode: KeyMode) -> Encoded {
    let m = k.mods;
    let any_mod = m.shift || m.alt || m.ctrl;
    let csi = |final_byte: char| -> Vec<u8> {
        if any_mod {
            format!("\x1b[1;{}{}", mod_param(m), final_byte).into_bytes()
        } else if mode.app_cursor {
            format!("\x1bO{final_byte}").into_bytes()
        } else {
            format!("\x1b[{final_byte}").into_bytes()
        }
    };
    let ss3 = |final_byte: char| -> Vec<u8> {
        if any_mod {
            format!("\x1b[1;{}{}", mod_param(m), final_byte).into_bytes()
        } else {
            format!("\x1bO{final_byte}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if any_mod {
            format!("\x1b[{};{}~", n, mod_param(m)).into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };
    let b = |s: &[u8]| Encoded::bytes(s);
    match k.key {
        // Umschalt/Strg/Alt+Enter: ESC CR – Claude Code nimmt das als Zeilenumbruch (meta+enter).
        Key::Enter => b(if any_mod { b"\x1b\r" } else { b"\r" }),
        // Strg+Rück wie Windows Terminal: 0x08 (conhost macht daraus Strg+Rück für PSReadLine).
        Key::Backspace => b(if m.ctrl { b"\x08" } else if m.alt { b"\x1b\x7f" } else { b"\x7f" }),
        Key::Tab => b(if m.shift { b"\x1b[Z" } else { b"\t" }),
        Key::Escape => b(b"\x1b"),
        Key::ArrowUp => Encoded::bytes(csi('A')),
        Key::ArrowDown => Encoded::bytes(csi('B')),
        Key::ArrowRight => Encoded::bytes(csi('C')),
        Key::ArrowLeft => Encoded::bytes(csi('D')),
        Key::Home => Encoded::bytes(csi('H')),
        Key::End => Encoded::bytes(csi('F')),
        Key::Insert => Encoded::bytes(tilde(2)),
        Key::Delete => Encoded::bytes(tilde(3)),
        Key::PageUp => Encoded::bytes(tilde(5)),
        Key::PageDown => Encoded::bytes(tilde(6)),
        Key::F1 => Encoded::bytes(ss3('P')),
        Key::F2 => Encoded::bytes(ss3('Q')),
        Key::F3 => Encoded::bytes(ss3('R')),
        Key::F4 => Encoded::bytes(ss3('S')),
        Key::F5 => Encoded::bytes(tilde(15)),
        Key::F6 => Encoded::bytes(tilde(17)),
        Key::F7 => Encoded::bytes(tilde(18)),
        Key::F8 => Encoded::bytes(tilde(19)),
        Key::F9 => Encoded::bytes(tilde(20)),
        Key::F10 => Encoded::bytes(tilde(21)),
        Key::F11 => Encoded::bytes(tilde(23)),
        Key::F12 => Encoded::bytes(tilde(24)),
        Key::F13 => Encoded::bytes(tilde(25)),
        Key::F14 => Encoded::bytes(tilde(26)),
        Key::F15 => Encoded::bytes(tilde(28)),
        Key::F16 => Encoded::bytes(tilde(29)),
        Key::F17 => Encoded::bytes(tilde(31)),
        Key::F18 => Encoded::bytes(tilde(32)),
        Key::F19 => Encoded::bytes(tilde(33)),
        Key::F20 => Encoded::bytes(tilde(34)),
        _ if altgr(m) => Encoded::default(), // AltGr-Zeichen kommen als Text
        Key::Space if m.ctrl => b(&[0]),
        Key::OpenBracket if m.ctrl => b(&[0x1b]),
        Key::Backslash if m.ctrl => b(&[0x1c]),
        Key::CloseBracket if m.ctrl => b(&[0x1d]),
        Key::Slash if m.ctrl => b(&[0x1f]),
        key if m.ctrl && is_letter(key) => {
            let c = key_char(key).unwrap_or('a') as u8;
            b(&[c - b'a' + 1])
        }
        // Reines Alt + Zeichentaste (Buchstaben, Ziffern, Satzzeichen, Leertaste): ESC + Zeichen.
        _ if m.alt => match typed_char(k) {
            Some(c) => {
                let mut v = vec![0x1b];
                let mut buf = [0u8; 4];
                v.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                Encoded::with_text(v)
            }
            None => Encoded::default(),
        },
        _ => Encoded::default(),
    }
}

/// Ziffernblock: kitty mit Flag 8 eigene Codes, win32-input VK_NUMPADx, DECKPAM `ESC O p` …, sonst das Zeichen.
fn numpad(c: char, mods: Modifiers, mode: KeyMode) -> Option<Vec<u8>> {
    let (vk, sc, kp_code, ss3) = match c {
        '0'..='9' => {
            let n = c as u8 - b'0';
            const SC: [u8; 10] = [0x52, 0x4F, 0x50, 0x51, 0x4B, 0x4C, 0x4D, 0x47, 0x48, 0x49];
            (0x60 + n as u16, SC[n as usize] as u16, 57399 + n as u32, (b'p' + n) as char)
        }
        '.' | ',' => (0x6E, 0x53, 57409, 'n'),
        '/' => (0x6F, 0x35, 57410, 'o'),
        '*' => (0x6A, 0x37, 57411, 'j'),
        '-' => (0x6D, 0x4A, 57412, 'm'),
        '+' => (0x6B, 0x4E, 57413, 'k'),
        _ => return None,
    };
    if mode.kitty_on() {
        if mode.kitty & KITTY_ALL_AS_ESC != 0 {
            let text = (mode.kitty & KITTY_TEXT != 0).then_some(c);
            return Some(csi_u(kp_code, None, None, mod_param(mods), Phase::Press, text, mode));
        }
        return Some(c.to_string().into_bytes());
    }
    if mode.win32 {
        // NUMLOCK_ON (0x20); die Divisionstaste ist eine erweiterte Taste (0x100).
        let cs = 0x20 | if mods.shift { 0x10 } else { 0 } | if vk == 0x6F { 0x100 } else { 0 };
        return Some(win32_seq(vk, sc, c as u32, cs));
    }
    if mode.app_keypad && !mods.shift {
        return Some(format!("\x1bO{ss3}").into_bytes());
    }
    Some(c.to_string().into_bytes())
}

// ───────────────────────────── win32-input-mode ─────────────────────────────

/// `CSI Vk;Sc;Uc;Kd;Cs;Rc _` – nur Tastendruck (Kd=1), Wiederholung 1.
fn win32_seq(vk: u16, sc: u16, uc: u32, cs: u32) -> Vec<u8> {
    format!("\x1b[{vk};{sc};{uc};1;{cs};1_").into_bytes()
}

/// dwControlKeyState: SHIFT_PRESSED 0x10, LEFT_CTRL_PRESSED 0x08, LEFT_ALT_PRESSED 0x02.
fn win32_state(m: Modifiers) -> u32 {
    (if m.shift { 0x10 } else { 0 }) | (if m.ctrl { 0x08 } else { 0 }) | (if m.alt { 0x02 } else { 0 })
}

/// Scancode (Satz 1) einer Buchstaben- oder Zifferntaste nach US-Position.
fn scan_code(key: Key) -> Option<u16> {
    let c = key_char(key)?;
    const LETTERS: &[u8; 26] = &[
        0x1E, 0x30, 0x2E, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32, 0x31, 0x18, 0x19, 0x10, 0x13, 0x1F, 0x14, 0x16, 0x2F, 0x11, 0x2D,
        0x15, 0x2C,
    ];
    Some(match c {
        'a'..='z' => LETTERS[(c as u8 - b'a') as usize] as u16,
        '1'..='9' => (c as u8 - b'1' + 2) as u16,
        '0' => 0x0B,
        _ => return None,
    })
}

/// Tasten, die unter win32-input-mode als echtes Tastenereignis gehen. Die Liste folgt der Sondierung (#183): nur wo
/// klassische Programme gewinnen und Programme mit VT-Eingabe (conhost-Übersetzung) dasselbe wie bisher bekommen.
fn win32_key(k: &KeyPress) -> Option<Vec<u8>> {
    let m = k.mods;
    if altgr(m) {
        return None;
    }
    let cs = win32_state(m);
    match k.key {
        // Strg+Enter: Zeichen LF wie unter Windows; PSReadLine bekommt Strg+Enter, VT-Programme `\n` (Claude Code:
        // ctrl+j = Zeilenumbruch). Umschalt+Enter NICHT: OpenConsole macht daraus für VT-Programme `\r`.
        Key::Enter if m.ctrl => Some(win32_seq(0x0D, 0x1C, 0x0A, cs)),
        // Strg+Leertaste: klassisch sonst Strg+Umschalt+2; VT-Programme bekommen in beiden Fällen NUL.
        Key::Space if m.ctrl => Some(win32_seq(0x20, 0x39, 0x20, cs)),
        // Strg+/: klassisch sonst Strg+Umschalt+-; VT-Programme bekommen 0x1F wie bisher.
        Key::Slash if m.ctrl => Some(win32_seq(0xBF, 0x35, 0x1F, cs)),
        Key::Tab if m.ctrl => Some(win32_seq(0x09, 0x0F, 0x09, cs)),
        Key::Escape => Some(win32_seq(0x1B, 0x01, 0x1B, cs)),
        // Strg+Ziffer: klassisch die echte Taste, VT-Programme die Ziffer (vorher: nichts).
        key if m.ctrl && !m.alt && !k.numpad && matches!(key_char(key), Some('0'..='9')) => {
            let c = key_char(key)? as u16;
            Some(win32_seq(c - b'0' as u16 + 0x30, scan_code(key)?, 0, cs))
        }
        // Strg+Umschalt+Buchstabe: klassisch unterscheidbar von Strg+Buchstabe, VT-Programme das Steuerzeichen.
        key if m.ctrl && m.shift && !m.alt && is_letter(key) => {
            let c = key_char(key)? as u8;
            let sc = k.physical.filter(|p| is_letter(*p)).and_then(scan_code).or_else(|| scan_code(key))?;
            Some(win32_seq(c.to_ascii_uppercase() as u16, sc, (c - b'a' + 1) as u32, cs))
        }
        _ => None,
    }
}

// ───────────────────────────── kitty (CSI u) ─────────────────────────────

/// `CSI code[:shifted[:base]][;mods[:event]][;text] u`
fn csi_u(code: u32, shifted: Option<u32>, base: Option<u32>, mods: u8, phase: Phase, text: Option<char>, mode: KeyMode) -> Vec<u8> {
    let mut s = format!("\x1b[{code}");
    if mode.kitty & KITTY_ALTERNATE_KEYS != 0 && (shifted.is_some() || base.is_some()) {
        s.push(':');
        if let Some(sh) = shifted {
            s.push_str(&sh.to_string());
        }
        if let Some(b) = base {
            s.push(':');
            s.push_str(&b.to_string());
        }
    }
    let ev = event_suffix(phase, mode);
    if mods > 1 || !ev.is_empty() || text.is_some() {
        s.push_str(&format!(";{mods}{ev}"));
    }
    if let Some(t) = text {
        s.push_str(&format!(";{}", t as u32));
    }
    s.push('u');
    s.into_bytes()
}

fn event_suffix(phase: Phase, mode: KeyMode) -> &'static str {
    if mode.kitty & KITTY_EVENT_TYPES == 0 {
        return "";
    }
    match phase {
        Phase::Press => "",
        Phase::Repeat => ":2",
        Phase::Release => ":3",
    }
}

/// Funktionstasten mit Buchstaben-Abschluss (`CSI 1;mods X`) bzw. Tilde (`CSI n;mods ~`).
fn kitty_functional(key: Key) -> Option<(u32, char)> {
    use Key::*;
    Some(match key {
        ArrowUp => (1, 'A'),
        ArrowDown => (1, 'B'),
        ArrowRight => (1, 'C'),
        ArrowLeft => (1, 'D'),
        Home => (1, 'H'),
        End => (1, 'F'),
        F1 => (1, 'P'),
        F2 => (1, 'Q'),
        F4 => (1, 'S'),
        Insert => (2, '~'),
        Delete => (3, '~'),
        PageUp => (5, '~'),
        PageDown => (6, '~'),
        F3 => (13, '~'),
        F5 => (15, '~'),
        F6 => (17, '~'),
        F7 => (18, '~'),
        F8 => (19, '~'),
        F9 => (20, '~'),
        F10 => (21, '~'),
        F11 => (23, '~'),
        F12 => (24, '~'),
        _ => return None,
    })
}

/// F13…F35: eigene Codes im privaten Unicode-Bereich (57376 …).
fn kitty_f_high(key: Key) -> Option<u32> {
    let n = key.name();
    let num: u32 = n.strip_prefix('F')?.parse().ok()?;
    (13..=35).contains(&num).then(|| 57376 + num - 13)
}

fn kitty_key(k: &KeyPress, mode: KeyMode) -> Encoded {
    let all = mode.kitty & KITTY_ALL_AS_ESC != 0;
    let m = k.mods;
    let mp = mod_param(m);
    let ev = event_suffix(k.phase, mode);
    // Loslassen nur mit Flag 2 (und dann nur für Tasten, die als Escape-Code gemeldet werden – siehe unten).
    if k.phase == Phase::Release && mode.kitty & KITTY_EVENT_TYPES == 0 {
        return Encoded::default();
    }
    if let Some((n, fin)) = kitty_functional(k.key) {
        let unmodified = mp == 1 && ev.is_empty();
        let s = if fin == '~' {
            if unmodified { format!("\x1b[{n}~") } else { format!("\x1b[{n};{mp}{ev}~") }
        } else if unmodified {
            // Pfeile/Pos1/Ende folgen DECCKM, F1/F2/F4 bleiben SS3 wie bei kitty.
            let ss3 = matches!(k.key, Key::F1 | Key::F2 | Key::F4) || mode.app_cursor;
            if ss3 { format!("\x1bO{fin}") } else { format!("\x1b[{fin}") }
        } else {
            format!("\x1b[1;{mp}{ev}{fin}")
        };
        return Encoded::bytes(s);
    }
    if let Some(code) = kitty_f_high(k.key) {
        return Encoded::bytes(csi_u(code, None, None, mp, k.phase, None, mode));
    }
    let special = match k.key {
        Key::Escape => Some((27u32, None)),
        Key::Enter => Some((13, Some(b"\r" as &[u8]))),
        Key::Tab => Some((9, Some(b"\t" as &[u8]))),
        Key::Backspace => Some((127, Some(b"\x7f" as &[u8]))),
        _ => None,
    };
    if let Some((code, legacy)) = special {
        if let Some(legacy) = legacy {
            // Enter/Tab/Rück ohne Modifikatoren bleiben klassisch (sonst kommt man nach einem Absturz des Programms
            // nicht mehr aus der Shell), Loslassen wird dann nicht gemeldet. Mit Flag 8 alles als CSI u.
            if !all && mp == 1 {
                return if k.phase == Phase::Release { Encoded::default() } else { Encoded::bytes(legacy) };
            }
        }
        return Encoded::bytes(csi_u(code, None, None, mp, k.phase, None, mode));
    }
    // Ziffernblock
    if k.numpad && all {
        if let Some(c) = single_char(k.text).or_else(|| key_char(k.key)) {
            if let Some(b) = numpad(c, m, mode) {
                return Encoded::with_text(b);
            }
        }
    }
    // Zeichentasten
    let Some(logical) = key_char(k.key) else { return Encoded::default() };
    if altgr(m) {
        return Encoded::default(); // AltGr-Zeichen kommen als Text
    }
    if !all && !m.ctrl && !m.alt {
        return Encoded::default(); // Text (auch mit Umschalt) bleibt Text
    }
    // Code = Taste ohne Umschalt. Bei umgeschalteten Symbolen (Umschalt+1 = '!') die Ziffer der Taste nehmen.
    let typed = single_char(k.text);
    let (code, shifted) = if m.shift && !logical.is_ascii_lowercase() && k.physical.and_then(key_char).is_some_and(|p| p.is_ascii_digit()) {
        (k.physical.and_then(key_char).unwrap_or(logical), Some(logical))
    } else if m.shift && logical.is_ascii_lowercase() {
        (logical, Some(typed.unwrap_or(logical.to_ascii_uppercase())))
    } else {
        (logical, if m.shift { typed } else { None })
    };
    let base = k.physical.and_then(key_char).filter(|p| *p != code);
    let text = if all && mode.kitty & KITTY_TEXT != 0 && !m.ctrl && !m.alt { typed } else { None };
    let shifted = shifted.filter(|s| *s != code).map(|c| c as u32);
    Encoded::with_text(csi_u(code as u32, shifted, base.map(|c| c as u32), mp, k.phase, text, mode))
}

// ───────────────────────────── DECSET 9001 im Ausgabestrom ─────────────────────────────

/// Erkennt `CSI ? … 9001 … h/l` (win32-input-mode) im Ausgabestrom, auch über Chunk-Grenzen. alacritty kennt den
/// Modus nicht; ConPTY schaltet ihn beim Start ein und beim Beenden wieder aus.
#[derive(Default)]
pub struct Win32ModeScanner {
    state: u8,
    params: Vec<u8>,
}

impl Win32ModeScanner {
    /// Neuer Zustand nach `bytes` (`None` = keine Änderung in diesem Chunk).
    pub fn feed(&mut self, bytes: &[u8]) -> Option<bool> {
        let mut out = None;
        for &b in bytes {
            match (self.state, b) {
                (_, 0x1b) => {
                    self.state = 1;
                    self.params.clear();
                }
                (1, b'[') => self.state = 2,
                (2, b'?') => self.state = 3,
                (3, b'0'..=b'9' | b';') if self.params.len() < 64 => self.params.push(b),
                (3, b'h' | b'l') => {
                    if self.params.split(|c| *c == b';').any(|p| p == b"9001") {
                        out = Some(b == b'h');
                    }
                    self.state = 0;
                }
                _ => self.state = 0,
            }
        }
        out
    }
}

// ───────────────────────────── Tastennamen (MCP send_keys) ─────────────────────────────

/// Tastenname wie „shift+enter“, „ctrl+backspace“, „alt+.“ in Bytes übersetzen – mit derselben Kodierung wie die
/// Tastatur. Modifikatoren: ctrl, alt, shift (je einmal). Tasten: enter/return, esc/escape, tab, backspace,
/// delete/del, insert/ins, up, down, left, right, home, end, pageup, pagedown, space, f1…f20, a…z, 0…9 und die
/// Satzzeichen `. , / \ - = ; ' [ ] \``. Unbekannt oder ohne Bytes (z. B. Strg+Alt+Buchstabe = AltGr) → `None`.
pub fn key_bytes(name: &str, mode: KeyMode) -> Option<Vec<u8>> {
    let n = name.trim().to_lowercase();
    let mut mods = Modifiers::NONE;
    let mut rest = n.as_str();
    loop {
        let (m, r) = if let Some(r) = rest.strip_prefix("ctrl+") {
            (&mut mods.ctrl, r)
        } else if let Some(r) = rest.strip_prefix("alt+") {
            (&mut mods.alt, r)
        } else if let Some(r) = rest.strip_prefix("shift+") {
            (&mut mods.shift, r)
        } else {
            break;
        };
        if *m || r.is_empty() {
            return None;
        }
        *m = true;
        rest = r;
    }
    mods.command = mods.ctrl;
    let key = match rest {
        "enter" | "return" => Key::Enter,
        "esc" | "escape" => Key::Escape,
        "tab" => Key::Tab,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "insert" | "ins" => Key::Insert,
        "up" => Key::ArrowUp,
        "down" => Key::ArrowDown,
        "left" => Key::ArrowLeft,
        "right" => Key::ArrowRight,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "space" => Key::Space,
        "." => Key::Period,
        "," => Key::Comma,
        "/" => Key::Slash,
        "\\" => Key::Backslash,
        "-" => Key::Minus,
        "=" => Key::Equals,
        ";" => Key::Semicolon,
        "'" => Key::Quote,
        "[" => Key::OpenBracket,
        "]" => Key::CloseBracket,
        "`" => Key::Backtick,
        f if f.starts_with('f') && f.len() <= 3 && f.len() > 1 => {
            let num: u8 = f[1..].parse().ok()?;
            if !(1..=20).contains(&num) {
                return None;
            }
            Key::from_name(&format!("F{num}"))?
        }
        c if c.len() == 1 && c.as_bytes()[0].is_ascii_alphanumeric() => Key::from_name(&c.to_ascii_uppercase())?,
        _ => return None,
    };
    // Text, den die Taste tippen würde (Umschalt nur für Buchstaben bekannt; mit Strg keiner).
    let text: Option<String> = key_char(key).filter(|_| !mods.ctrl).and_then(|c| {
        if c.is_ascii_lowercase() {
            Some(if mods.shift { c.to_ascii_uppercase() } else { c }.to_string())
        } else if mods.shift {
            None
        } else {
            Some(c.to_string())
        }
    });
    let kp = KeyPress { key, physical: Some(key), mods, phase: Phase::Press, numpad: false, text: text.as_deref() };
    let enc = encode_key(&kp, mode);
    let bytes = if enc.eat_text || text.is_none() {
        enc.bytes
    } else {
        let mut b = enc.bytes;
        b.extend(encode_text(text.as_deref().unwrap_or_default(), mods, false, mode));
        b
    };
    (!bytes.is_empty()).then_some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEGACY: KeyMode = KeyMode { app_cursor: false, app_keypad: false, win32: false, kitty: 0 };
    const WIN32: KeyMode = KeyMode { app_cursor: false, app_keypad: false, win32: true, kitty: 0 };

    fn kitty(flags: u8) -> KeyMode {
        // kitty schlägt win32-input: ConPTY hat 9001 immer an.
        KeyMode { app_cursor: false, app_keypad: false, win32: true, kitty: flags }
    }

    /// Modifikatoren aus Buchstaben: s = Umschalt, a = Alt, c = Strg.
    fn mods(s: &str) -> Modifiers {
        Modifiers { shift: s.contains('s'), alt: s.contains('a'), ctrl: s.contains('c'), command: s.contains('c'), mac_cmd: false }
    }

    fn key(k: Key, m: &str, mode: KeyMode) -> Vec<u8> {
        encode_key(&KeyPress::new(k, mods(m)), mode).bytes
    }

    /// Taste mit dem Text, den egui danach als `Event::Text` liefert; gesendet wird, was der Bildschirm-Code sendet.
    fn typed(k: Key, physical: Key, m: &str, text: &str, mode: KeyMode) -> Vec<u8> {
        let kp = KeyPress { key: k, physical: Some(physical), mods: mods(m), phase: Phase::Press, numpad: false, text: Some(text) };
        let e = encode_key(&kp, mode);
        let mut out = e.bytes;
        if !e.eat_text {
            out.extend(encode_text(text, mods(m), false, mode));
        }
        out
    }

    fn esc(b: &[u8]) -> String {
        b.iter()
            .map(|&c| match c {
                0x1b => "\\e".to_string(),
                b'\r' => "\\r".to_string(),
                b'\t' => "\\t".to_string(),
                b'\n' => "\\n".to_string(),
                0x20..=0x7e => (c as char).to_string(),
                _ => format!("\\x{c:02x}"),
            })
            .collect()
    }

    fn check(table: &[(Key, &str, KeyMode, &str)]) {
        for (k, m, mode, want) in table {
            assert_eq!(esc(&key(*k, m, *mode)), *want, "{k:?} mods={m:?} mode={mode:?}");
        }
    }

    #[test]
    fn enter_variants() {
        check(&[
            (Key::Enter, "", LEGACY, "\\r"),
            (Key::Enter, "s", LEGACY, "\\e\\r"),
            (Key::Enter, "c", LEGACY, "\\e\\r"),
            (Key::Enter, "a", LEGACY, "\\e\\r"),
            (Key::Enter, "", WIN32, "\\r"),
            // Umschalt+Enter bleibt ESC CR: OpenConsole übersetzt die win32-Taste für VT-Programme in ein nacktes CR.
            (Key::Enter, "s", WIN32, "\\e\\r"),
            (Key::Enter, "a", WIN32, "\\e\\r"),
            (Key::Enter, "c", WIN32, "\\e[13;28;10;1;8;1_"),
            (Key::Enter, "cs", WIN32, "\\e[13;28;10;1;24;1_"),
            (Key::Enter, "", kitty(1), "\\r"),
            (Key::Enter, "s", kitty(1), "\\e[13;2u"),
            (Key::Enter, "c", kitty(1), "\\e[13;5u"),
            (Key::Enter, "a", kitty(1), "\\e[13;3u"),
            (Key::Enter, "s", kitty(5), "\\e[13;2u"),
            (Key::Enter, "", kitty(2 | 1), "\\r"),
            (Key::Enter, "s", kitty(2 | 1), "\\e[13;2u"),
            (Key::Enter, "", kitty(8), "\\e[13u"),
            (Key::Enter, "", kitty(8 | 2), "\\e[13u"),
            (Key::Enter, "s", kitty(31), "\\e[13;2u"),
        ]);
    }

    #[test]
    fn backspace_variants() {
        check(&[
            (Key::Backspace, "", LEGACY, "\\x7f"),
            (Key::Backspace, "c", LEGACY, "\\x08"),
            (Key::Backspace, "a", LEGACY, "\\e\\x7f"),
            (Key::Backspace, "c", WIN32, "\\x08"),
            (Key::Backspace, "a", WIN32, "\\e\\x7f"),
            (Key::Backspace, "", kitty(1), "\\x7f"),
            (Key::Backspace, "c", kitty(1), "\\e[127;5u"),
            (Key::Backspace, "a", kitty(1), "\\e[127;3u"),
            (Key::Backspace, "", kitty(8), "\\e[127u"),
        ]);
    }

    #[test]
    fn tab_escape_space_slash() {
        check(&[
            (Key::Tab, "", LEGACY, "\\t"),
            (Key::Tab, "s", LEGACY, "\\e[Z"),
            (Key::Tab, "s", WIN32, "\\e[Z"),
            (Key::Tab, "c", WIN32, "\\e[9;15;9;1;8;1_"),
            (Key::Tab, "s", kitty(1), "\\e[9;2u"),
            (Key::Escape, "", LEGACY, "\\e"),
            (Key::Escape, "", WIN32, "\\e[27;1;27;1;0;1_"),
            (Key::Escape, "", kitty(1), "\\e[27u"),
            (Key::Escape, "s", kitty(1), "\\e[27;2u"),
            (Key::Space, "c", LEGACY, "\\x00"),
            (Key::Space, "c", WIN32, "\\e[32;57;32;1;8;1_"),
            (Key::Space, "c", kitty(1), "\\e[32;5u"),
            (Key::Space, "", kitty(1), ""), // Text
            (Key::Slash, "c", LEGACY, "\\x1f"),
            (Key::Slash, "c", WIN32, "\\e[191;53;31;1;8;1_"),
            (Key::Slash, "c", kitty(1), "\\e[47;5u"),
            (Key::OpenBracket, "c", LEGACY, "\\e"),
            (Key::Backslash, "c", WIN32, "\\x1c"),
            (Key::CloseBracket, "c", LEGACY, "\\x1d"),
        ]);
    }

    #[test]
    fn ctrl_letters_and_digits() {
        check(&[
            (Key::C, "c", LEGACY, "\\x03"),
            (Key::C, "c", WIN32, "\\x03"),
            (Key::C, "c", kitty(1), "\\e[99;5u"),
            (Key::A, "cs", LEGACY, "\\x01"),
            (Key::A, "cs", WIN32, "\\e[65;30;1;1;24;1_"),
            (Key::A, "cs", kitty(1), "\\e[97;6u"),
            (Key::A, "cs", kitty(5), "\\e[97:65;6u"),
            (Key::Num1, "c", LEGACY, ""),
            (Key::Num1, "c", WIN32, "\\e[49;2;0;1;8;1_"),
            (Key::Num1, "c", kitty(1), "\\e[49;5u"),
            // Strg+Alt+Buchstabe = AltGr: nie aus dem Tastenereignis
            (Key::Q, "ca", LEGACY, ""),
            (Key::Q, "ca", WIN32, ""),
            (Key::Q, "ca", kitty(1), ""),
        ]);
    }

    #[test]
    fn alt_digits_and_punctuation() {
        // Alt + Taste: ESC + Zeichen (vorher kam bei Ziffern und Satzzeichen nichts an).
        assert_eq!(esc(&typed(Key::Period, Key::Period, "a", ".", LEGACY)), "\\e.");
        assert_eq!(esc(&typed(Key::Period, Key::Period, "a", ".", WIN32)), "\\e.");
        assert_eq!(esc(&typed(Key::Num1, Key::Num1, "a", "1", WIN32)), "\\e1");
        assert_eq!(esc(&typed(Key::B, Key::B, "a", "b", LEGACY)), "\\eb");
        assert_eq!(esc(&typed(Key::B, Key::B, "as", "B", LEGACY)), "\\eB");
        // ohne folgenden Text (egui liefert ihn nicht immer): aus der Taste
        check(&[(Key::Period, "a", LEGACY, "\\e."), (Key::Num7, "a", WIN32, "\\e7"), (Key::F, "a", LEGACY, "\\ef")]);
        // kitty
        assert_eq!(esc(&typed(Key::Period, Key::Period, "a", ".", kitty(1))), "\\e[46;3u");
        assert_eq!(esc(&typed(Key::Num1, Key::Num1, "a", "1", kitty(1))), "\\e[49;3u");
        // Alt + Zeichen ohne egui-Taste (deutsches ö): nur Text-Ereignis
        assert_eq!(esc(&encode_text("ö", mods("a"), false, LEGACY)), "\\e\\xc3\\xb6");
        assert_eq!(esc(&encode_text("ö", mods("a"), false, kitty(1))), "\\e[246;3u");
        // Umschalt+1 = '!' mit kitty: Code ist die Ziffer, '!' die umgeschaltete Taste
        assert_eq!(esc(&typed(Key::Exclamationmark, Key::Num1, "as", "!", kitty(5))), "\\e[49:33;4u");
    }

    #[test]
    fn function_keys_with_modifiers() {
        check(&[
            (Key::F1, "", LEGACY, "\\eOP"),
            (Key::F1, "s", LEGACY, "\\e[1;2P"),
            (Key::F3, "", LEGACY, "\\eOR"),
            (Key::F5, "", LEGACY, "\\e[15~"),
            (Key::F5, "c", LEGACY, "\\e[15;5~"),
            (Key::F5, "c", WIN32, "\\e[15;5~"),
            (Key::F12, "csa", LEGACY, "\\e[24;8~"),
            (Key::F13, "", LEGACY, "\\e[25~"),
            (Key::F1, "", kitty(1), "\\eOP"),
            (Key::F1, "c", kitty(1), "\\e[1;5P"),
            (Key::F3, "", kitty(1), "\\e[13~"),
            (Key::F3, "s", kitty(1), "\\e[13;2~"),
            (Key::F5, "a", kitty(1), "\\e[15;3~"),
            (Key::F13, "", kitty(1), "\\e[57376u"),
            (Key::F20, "s", kitty(1), "\\e[57383;2u"),
        ]);
    }

    #[test]
    fn arrows_and_navigation() {
        let app = KeyMode { app_cursor: true, ..LEGACY };
        check(&[
            (Key::ArrowUp, "", LEGACY, "\\e[A"),
            (Key::ArrowUp, "", app, "\\eOA"),
            (Key::ArrowUp, "s", app, "\\e[1;2A"),
            (Key::ArrowLeft, "c", LEGACY, "\\e[1;5D"),
            (Key::ArrowLeft, "c", WIN32, "\\e[1;5D"),
            (Key::Home, "", LEGACY, "\\e[H"),
            (Key::End, "s", LEGACY, "\\e[1;2F"),
            (Key::Delete, "", LEGACY, "\\e[3~"),
            (Key::PageUp, "c", LEGACY, "\\e[5;5~"),
            (Key::ArrowUp, "", kitty(1), "\\e[A"),
            (Key::ArrowUp, "", KeyMode { app_cursor: true, ..kitty(1) }, "\\eOA"),
            (Key::ArrowUp, "cs", kitty(1), "\\e[1;6A"),
            (Key::Delete, "", kitty(1), "\\e[3~"),
            (Key::Delete, "a", kitty(1), "\\e[3;3~"),
        ]);
    }

    #[test]
    fn kitty_event_types() {
        let ev = kitty(1 | 2);
        let press = |k: Key, m: &str, phase: Phase| {
            let kp = KeyPress { phase, ..KeyPress::new(k, mods(m)) };
            esc(&encode_key(&kp, ev).bytes)
        };
        assert_eq!(press(Key::ArrowUp, "", Phase::Press), "\\e[A");
        assert_eq!(press(Key::ArrowUp, "", Phase::Repeat), "\\e[1;1:2A");
        assert_eq!(press(Key::ArrowUp, "", Phase::Release), "\\e[1;1:3A");
        assert_eq!(press(Key::Escape, "", Phase::Release), "\\e[27;1:3u");
        assert_eq!(press(Key::A, "c", Phase::Release), "\\e[97;5:3u");
        assert_eq!(press(Key::Delete, "", Phase::Release), "\\e[3;1:3~");
        // Enter ohne Modifikatoren ist klassisch, sein Loslassen wird nicht gemeldet
        assert_eq!(press(Key::Enter, "", Phase::Release), "");
        // ohne Flag 2 kein Loslassen
        let kp = KeyPress { phase: Phase::Release, ..KeyPress::new(Key::Escape, mods("")) };
        assert!(encode_key(&kp, kitty(1)).bytes.is_empty());
        assert!(encode_key(&kp, WIN32).bytes.is_empty());
    }

    #[test]
    fn kitty_all_keys_and_text() {
        // Flag 8: Text ohne Modifikatoren als CSI u, Flag 16 hängt den Text an.
        assert_eq!(esc(&typed(Key::A, Key::A, "", "a", kitty(8))), "\\e[97u");
        assert_eq!(esc(&typed(Key::A, Key::A, "s", "A", kitty(8 | 4))), "\\e[97:65;2u");
        assert_eq!(esc(&typed(Key::A, Key::A, "", "a", kitty(8 | 16))), "\\e[97;1;97u");
        assert_eq!(esc(&typed(Key::A, Key::A, "s", "A", kitty(8 | 16))), "\\e[97;2;65u");
        // Flag 1: Text bleibt Text
        assert_eq!(esc(&typed(Key::A, Key::A, "", "a", kitty(1))), "a");
        assert_eq!(esc(&typed(Key::A, Key::A, "s", "A", kitty(1))), "A");
        // Deutsche Belegung: logisch Z auf der US-Position Y – Grundbelegung mit Flag 4
        assert_eq!(esc(&typed(Key::Z, Key::Y, "c", "z", kitty(1 | 4))), "\\e[122::121;5u");
        assert_eq!(esc(&typed(Key::Z, Key::Y, "c", "z", kitty(1))), "\\e[122;5u");
    }

    #[test]
    fn numpad() {
        let np = |c: &str, mode: KeyMode| {
            let k = Key::from_name(c).unwrap_or(Key::Num0);
            let kp = KeyPress { numpad: true, text: Some(c), ..KeyPress::new(k, mods("")) };
            let e = encode_key(&kp, mode);
            let mut b = e.bytes;
            if !e.eat_text {
                b.extend(encode_text(c, mods(""), true, mode));
            }
            esc(&b)
        };
        let keypad = KeyMode { app_keypad: true, ..LEGACY };
        assert_eq!(np("5", LEGACY), "5");
        assert_eq!(np("5", keypad), "\\eOu");
        assert_eq!(np("0", keypad), "\\eOp");
        assert_eq!(np("5", WIN32), "\\e[101;76;53;1;32;1_");
        assert_eq!(np("5", KeyMode { app_keypad: true, ..WIN32 }), "\\e[101;76;53;1;32;1_");
        assert_eq!(np("5", kitty(1)), "5");
        assert_eq!(np("5", kitty(8)), "\\e[57404u");
        // '*' hat keine egui-Taste und kommt nur als Text
        assert_eq!(esc(&encode_text("*", mods(""), true, keypad)), "\\eOj");
        assert_eq!(esc(&encode_text("*", mods(""), true, WIN32)), "\\e[106;55;42;1;32;1_");
        assert_eq!(esc(&encode_text("/", mods(""), true, WIN32)), "\\e[111;53;47;1;288;1_");
        // Dezimaltrenner der deutschen Belegung
        assert_eq!(esc(&encode_text(",", mods(""), true, WIN32)), "\\e[110;83;44;1;32;1_");
        assert_eq!(esc(&encode_text("*", mods(""), false, keypad)), "*");
    }

    #[test]
    fn altgr_and_dead_keys_stay_text() {
        for mode in [LEGACY, WIN32, kitty(1), kitty(5)] {
            // AltGr+Q = @, AltGr+7 = {, AltGr+ß = \ (deutsch): Taste mit Strg+Alt, Zeichen als Text
            assert_eq!(esc(&typed(Key::Q, Key::Q, "ca", "@", mode)), "@", "{mode:?}");
            assert_eq!(esc(&typed(Key::Num7, Key::Num7, "ca", "{", mode)), "{", "{mode:?}");
            assert_eq!(esc(&encode_text("\\", mods("ca"), false, mode)), "\\", "{mode:?}");
            // Tottasten: kein Tastenereignis, nur das zusammengesetzte Zeichen
            assert_eq!(esc(&encode_text("ê", mods(""), false, mode)), "\\xc3\\xaa", "{mode:?}");
            assert_eq!(esc(&encode_text("^", mods(""), false, mode)), "^", "{mode:?}");
            assert_eq!(esc(&encode_text("é", mods("s"), false, mode)), "\\xc3\\xa9", "{mode:?}");
            // Steuerzeichen als Text nie
            assert!(encode_text("\u{7f}", mods(""), false, mode).is_empty());
        }
    }

    #[test]
    fn mode_from_term() {
        let m = KeyMode::new(TermMode::APP_CURSOR | TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_ALTERNATE_KEYS, true);
        assert_eq!(m, KeyMode { app_cursor: true, app_keypad: false, win32: true, kitty: 5 });
        assert!(m.kitty_on());
        assert!(!KeyMode { kitty: KITTY_EVENT_TYPES, ..LEGACY }.kitty_on());
    }

    #[test]
    fn win32_mode_scanner() {
        let mut s = Win32ModeScanner::default();
        assert_eq!(s.feed(b"\x1b[6n\x1b[?9001h\x1b[?1004h"), Some(true));
        assert_eq!(s.feed(b"hello \x1b[?25l"), None);
        // über Chunk-Grenzen und in einer Liste
        assert_eq!(s.feed(b"\x1b[?10"), None);
        assert_eq!(s.feed(b"04;9001l"), Some(false));
        assert_eq!(s.feed(b"\x1b[?90011h\x1b[9001h"), None);
        assert_eq!(s.feed(b"\x1b[?9001l\x1b[?9001h"), Some(true));
    }

    #[test]
    fn named_keys_for_agents() {
        // dieselben Bytes wie die Tastatur
        assert_eq!(key_bytes("Enter", LEGACY), Some(b"\r".to_vec()));
        assert_eq!(key_bytes("up", KeyMode { app_cursor: true, ..LEGACY }), Some(b"\x1bOA".to_vec()));
        assert_eq!(key_bytes("up", LEGACY), Some(b"\x1b[A".to_vec()));
        assert_eq!(key_bytes("ctrl+c", LEGACY), Some(vec![3]));
        assert_eq!(key_bytes("ctrl+c", WIN32), Some(vec![3]));
        assert_eq!(key_bytes("f5", LEGACY), Some(b"\x1b[15~".to_vec()));
        assert_eq!(key_bytes("f1", LEGACY), Some(b"\x1bOP".to_vec()));
        assert_eq!(key_bytes("shift+tab", LEGACY), Some(b"\x1b[Z".to_vec()));
        assert_eq!(key_bytes("backspace", LEGACY), Some(b"\x7f".to_vec()));
        assert_eq!(key_bytes("delete", LEGACY), Some(b"\x1b[3~".to_vec()));
        assert_eq!(key_bytes("ctrl+l", LEGACY), Some(vec![12]));
        assert_eq!(key_bytes("ctrl+z", LEGACY), Some(vec![26]));
        assert_eq!(key_bytes("ctrl+d", LEGACY), Some(vec![4]));
        assert_eq!(key_bytes("esc", LEGACY), Some(vec![0x1b]));
        // neu
        assert_eq!(key_bytes("shift+enter", WIN32), Some(b"\x1b\r".to_vec()));
        assert_eq!(key_bytes("shift+enter", kitty(5)), Some(b"\x1b[13;2u".to_vec()));
        assert_eq!(key_bytes("ctrl+backspace", WIN32), Some(vec![8]));
        assert_eq!(key_bytes("alt+.", WIN32), Some(b"\x1b.".to_vec()));
        assert_eq!(key_bytes("ALT+.", kitty(1)), Some(b"\x1b[46;3u".to_vec()));
        assert_eq!(key_bytes("ctrl+space", WIN32), Some(b"\x1b[32;57;32;1;8;1_".to_vec()));
        assert_eq!(key_bytes("esc", kitty(1)), Some(b"\x1b[27u".to_vec()));
        assert_eq!(key_bytes("ctrl+shift+a", WIN32), Some(b"\x1b[65;30;1;1;24;1_".to_vec()));
        assert_eq!(key_bytes("f20", LEGACY), Some(b"\x1b[34~".to_vec()));
        assert_eq!(key_bytes("shift+f20", kitty(1)), Some(b"\x1b[57383;2u".to_vec()));
        assert!(key_bytes("f21", LEGACY).is_none());
        assert_eq!(key_bytes("a", LEGACY), Some(b"a".to_vec()));
        assert_eq!(key_bytes("shift+a", LEGACY), Some(b"A".to_vec()));
        // abgelehnt
        assert!(key_bytes("ctrl+alt+entf", LEGACY).is_none());
        assert!(key_bytes("ctrl+alt+x", LEGACY).is_none()); // AltGr
        assert!(key_bytes("f99", LEGACY).is_none());
        assert!(key_bytes("f0", LEGACY).is_none());
        assert!(key_bytes("ctrl+ctrl+c", LEGACY).is_none());
        assert!(key_bytes("ctrl+", LEGACY).is_none());
        assert!(key_bytes("", LEGACY).is_none());
        assert!(key_bytes("shift+1", LEGACY).is_none()); // Zeichen hängt von der Belegung ab
    }
}
