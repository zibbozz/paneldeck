//! JSON und JSONC formatieren, minifizieren, prüfen und Schlüssel sortieren (#381).
//!
//! Aufbau – ohne Wertebaum und ohne Rekursion über die Schachtelungstiefe (10 000 Ebenen und 50 MB gehen):
//! - [`Lexer`]: handgeschriebener Tokenizer über die Bytes. Zahlen, Zeichenketten (samt Escapes), Literale und
//!   Kommentare bleiben Ausschnitte der Eingabe – nichts wird umgerechnet oder neu escaped (`1.0`, `1e400`,
//!   `0.1000`, `\u00e9` bleiben, wie sie sind). Doppelte Schlüssel bleiben erhalten.
//! - [`parse`]: prüft die Grammatik mit einem expliziten Stapel und reicht die Tokens an eine Senke weiter.
//!   Strenges JSON lehnt Kommentare und Komma am Ende mit Position ab; JSONC erlaubt beides.
//! - [`Emitter`]: schreibt formatiert oder minifiziert. Kommentare werden gepuffert und nach `,`/`:` bzw. vor dem
//!   nächsten Token gesetzt: ein Kommentar in eigener Zeile bleibt in eigener Zeile, ein Kommentar am Zeilenende
//!   bleibt am Zeilenende. Leere Objekte/Arrays werden `{}`/`[]`; eine Leerzeile zwischen Einträgen bzw. vor einem
//!   Kommentar bleibt erhalten (mehrere werden zu einer). Minifizieren lässt Kommentare und Komma am Ende weg.
//! - `SortKeys` sammelt die Tokens, bildet je Container die Einträge (mit ihren Kommentaren davor und am
//!   Zeilenende), sortiert die Objekteinträge stabil nach dem dekodierten Schlüssel (Unicode-Codepunkte,
//!   Groß-/Kleinschreibung zählt) und gibt den umgestellten Strom an den Emitter. Array-Reihenfolge bleibt.
//!
//! Ein Byte Order Mark am Anfang bleibt erhalten. Fehlerpositionen: Zeile/Spalte 1-basiert, Spalte in
//! Unicode-Zeichen (Tab = 1), gezählt im übergebenen Text. Fortschritt etwa je 1 % der Eingabe; `cancel` wird an
//! denselben Stellen geprüft.

use super::{FormatError, Indent, Op, Options, Outcome};
use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};

const BOM: char = '\u{feff}';

/// Kommen Kommentare (`//` oder `/* */` außerhalb von Zeichenketten) vor?
pub fn has_comments(text: &str) -> bool {
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' => {
                i += 1;
                while i < b.len() {
                    match b[i] {
                        b'\\' => i += 2,
                        // Kaputte Zeichenkette: am Zeilenende aufhören, damit der Rest noch geprüft wird.
                        b'"' | b'\n' => break,
                        _ => i += 1,
                    }
                }
                i += 1;
            }
            b'/' if matches!(b.get(i + 1), Some(b'/' | b'*')) => return true,
            _ => i += 1,
        }
    }
    false
}

/// Siehe [`super::run`].
pub fn run(jsonc: bool, op: Op, text: &str, opts: Options, cancel: &AtomicBool, progress: &dyn Fn(f32)) -> Outcome {
    if text.len() >= u32::MAX as usize {
        return Outcome::Invalid(FormatError { line: 1, column: 1, message: "input is too large (4 GiB or more)".into() });
    }
    let base = if text.starts_with(BOM) { BOM.len_utf8() } else { 0 };
    let mut tick = Ticker::new(cancel, progress);
    let res = match op {
        Op::Validate => {
            tick.phase(0.0, 1.0, text.len());
            parse(text, base, jsonc, &mut tick, &mut |_| {}).map(|()| text.to_string())
        }
        Op::Format | Op::Minify => {
            tick.phase(0.0, 1.0, text.len());
            let mut em = Emitter::new(text, base, op == Op::Minify, opts.indent, text.len());
            let r = parse(text, base, jsonc, &mut tick, &mut |t| em.push(t));
            r.map(|()| em.finish(opts.final_newline))
        }
        Op::SortKeys => sort_keys(text, base, jsonc, opts, &mut tick),
    };
    match res {
        Ok(s) => {
            tick.done();
            Outcome::Text(s)
        }
        Err(Stop::Cancelled) => Outcome::Cancelled,
        Err(Stop::Invalid(pos, message)) => {
            let (line, column) = line_col(text, pos);
            Outcome::Invalid(FormatError { line, column, message })
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Tokens, Fortschritt, Positionen

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum K {
    ObjOpen,
    ObjClose,
    ArrOpen,
    ArrClose,
    Colon,
    Comma,
    Str,
    Num,
    /// `true`, `false`, `null`.
    Lit,
    LineComment,
    BlockComment,
}

impl K {
    fn is_comment(self) -> bool {
        matches!(self, K::LineComment | K::BlockComment)
    }
    fn is_open(self) -> bool {
        matches!(self, K::ObjOpen | K::ArrOpen)
    }
}

/// Ein Token als Ausschnitt `start..end` (Bytes) der Eingabe; `nl_before`: davor stand ein Zeilenumbruch,
/// `blank_before`: davor stand eine Leerzeile.
#[derive(Clone, Copy, Debug)]
struct Tok {
    kind: K,
    start: u32,
    end: u32,
    nl_before: bool,
    blank_before: bool,
}

enum Stop {
    /// Byte-Position und Meldung.
    Invalid(usize, String),
    Cancelled,
}

fn inv(pos: usize, msg: impl Into<String>) -> Stop {
    Stop::Invalid(pos, msg.into())
}

/// Fortschritt (monoton steigend) und Abbruch, geprüft etwa je 1 % einer Phase.
struct Ticker<'a> {
    cancel: &'a AtomicBool,
    progress: &'a dyn Fn(f32),
    lo: f32,
    span: f32,
    total: usize,
    step: usize,
    next: usize,
    last: f32,
}

impl<'a> Ticker<'a> {
    fn new(cancel: &'a AtomicBool, progress: &'a dyn Fn(f32)) -> Self {
        progress(0.0);
        Ticker { cancel, progress, lo: 0.0, span: 1.0, total: 1, step: 1, next: 0, last: 0.0 }
    }
    fn phase(&mut self, lo: f32, hi: f32, total: usize) {
        self.lo = lo;
        self.span = hi - lo;
        self.total = total.max(1);
        self.step = (total / 100).max(1024);
        self.next = 0;
    }
    #[inline]
    fn at(&mut self, done: usize) -> Result<(), Stop> {
        if done >= self.next {
            self.check(done)
        } else {
            Ok(())
        }
    }
    #[cold]
    fn check(&mut self, done: usize) -> Result<(), Stop> {
        self.next = done.saturating_add(self.step);
        if self.cancel.load(Ordering::Relaxed) {
            return Err(Stop::Cancelled);
        }
        let f = self.lo + self.span * (done as f32 / self.total as f32).min(1.0);
        if f > self.last {
            self.last = f;
            (self.progress)(f);
        }
        Ok(())
    }
    fn done(&mut self) {
        if self.last < 1.0 {
            self.last = 1.0;
            (self.progress)(1.0);
        }
    }
}

/// Byte-Position → (Zeile, Spalte), beide 1-basiert, Spalte in Zeichen.
fn line_col(text: &str, pos: usize) -> (usize, usize) {
    let b = text.as_bytes();
    let mut pos = pos.min(b.len());
    while pos > 0 && !text.is_char_boundary(pos) {
        pos -= 1;
    }
    let before = &b[..pos];
    let line = before.iter().filter(|&&c| c == b'\n').count() + 1;
    let line_start = before.iter().rposition(|&c| c == b'\n').map_or(0, |p| p + 1);
    let column = before[line_start..].iter().filter(|&&c| (c & 0xC0) != 0x80).count() + 1;
    (line, column)
}

/// Zeichen an `pos` für eine Meldung: `'x'` oder `U+0007` bei Steuer-/Leerzeichen.
fn show_char(s: &str, pos: usize) -> String {
    match s.get(pos..).and_then(|r| r.chars().next()) {
        Some(c) if c.is_control() || c.is_whitespace() || c == BOM => format!("U+{:04X}", c as u32),
        Some(c) => format!("'{c}'"),
        None => "end of input".into(),
    }
}

fn describe(s: &str, t: Tok) -> String {
    match t.kind {
        K::ObjOpen => "'{'".into(),
        K::ObjClose => "'}'".into(),
        K::ArrOpen => "'['".into(),
        K::ArrClose => "']'".into(),
        K::Colon => "':'".into(),
        K::Comma => "','".into(),
        K::Str => "a string".into(),
        K::Num => "a number".into(),
        K::Lit => format!("'{}'", &s[t.start as usize..t.end as usize]),
        K::LineComment | K::BlockComment => "a comment".into(),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Lexer

struct Lexer<'a> {
    s: &'a str,
    b: &'a [u8],
    pos: usize,
    jsonc: bool,
}

impl<'a> Lexer<'a> {
    fn token(&mut self) -> Result<Option<Tok>, Stop> {
        let b = self.b;
        let mut nls = 0u8;
        while let Some(&c) = b.get(self.pos) {
            match c {
                b' ' | b'\t' | b'\r' => self.pos += 1,
                b'\n' => {
                    nls = nls.saturating_add(1);
                    self.pos += 1;
                }
                _ => break,
            }
        }
        let start = self.pos;
        let Some(&c) = b.get(start) else { return Ok(None) };
        let kind = match c {
            b'{' => K::ObjOpen,
            b'}' => K::ObjClose,
            b'[' => K::ArrOpen,
            b']' => K::ArrClose,
            b':' => K::Colon,
            b',' => K::Comma,
            b'"' => {
                self.string()?;
                K::Str
            }
            b'-' | b'0'..=b'9' => {
                self.number()?;
                K::Num
            }
            b'/' => return self.comment(start, nls).map(Some),
            b'a'..=b'z' | b'A'..=b'Z' | b'_' | b'$' => {
                self.word()?;
                K::Lit
            }
            b'\'' => return Err(inv(start, "strings must use double quotes (\")")),
            _ => return Err(inv(start, format!("unexpected character {}", show_char(self.s, start)))),
        };
        if matches!(kind, K::ObjOpen | K::ObjClose | K::ArrOpen | K::ArrClose | K::Colon | K::Comma) {
            self.pos += 1;
        }
        Ok(Some(Tok { kind, start: start as u32, end: self.pos as u32, nl_before: nls > 0, blank_before: nls > 1 }))
    }

    fn string(&mut self) -> Result<(), Stop> {
        let b = self.b;
        let start = self.pos;
        let mut i = start + 1;
        loop {
            match b.get(i) {
                None => return Err(inv(start, "unterminated string (missing closing '\"')")),
                Some(b'"') => {
                    self.pos = i + 1;
                    return Ok(());
                }
                Some(b'\\') => match b.get(i + 1) {
                    Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => i += 2,
                    Some(b'u') => {
                        if (0..4).any(|k| !b.get(i + 2 + k).is_some_and(u8::is_ascii_hexdigit)) {
                            return Err(inv(i, "invalid \\u escape in string: expected four hex digits"));
                        }
                        i += 6;
                    }
                    None => return Err(inv(start, "unterminated string (missing closing '\"')")),
                    Some(_) => {
                        return Err(inv(i, format!("invalid escape sequence '\\{}' in string", self.s[i + 1..].chars().next().unwrap_or(' '))))
                    }
                },
                Some(b'\n') => return Err(inv(i, "unterminated string: line break inside a string (use \\n)")),
                Some(&c) if c < 0x20 => {
                    return Err(inv(i, format!("control character U+{c:04X} in string must be escaped")));
                }
                Some(_) => i += 1,
            }
        }
    }

    fn number(&mut self) -> Result<(), Stop> {
        let b = self.b;
        let digit = |i: usize| b.get(i).is_some_and(u8::is_ascii_digit);
        let mut i = self.pos;
        if b[i] == b'-' {
            i += 1;
        }
        match b.get(i) {
            Some(b'0') => {
                i += 1;
                if digit(i) {
                    return Err(inv(i - 1, "leading zeros are not allowed in numbers"));
                }
            }
            Some(b'1'..=b'9') => {
                while digit(i) {
                    i += 1;
                }
            }
            _ => return Err(inv(i, "expected a digit after '-'")),
        }
        if b.get(i) == Some(&b'.') {
            i += 1;
            if !digit(i) {
                return Err(inv(i, "expected a digit after the decimal point"));
            }
            while digit(i) {
                i += 1;
            }
        }
        if matches!(b.get(i), Some(b'e' | b'E')) {
            i += 1;
            if matches!(b.get(i), Some(b'+' | b'-')) {
                i += 1;
            }
            if !digit(i) {
                return Err(inv(i, "expected a digit in the exponent"));
            }
            while digit(i) {
                i += 1;
            }
        }
        self.pos = i;
        Ok(())
    }

    fn word(&mut self) -> Result<(), Stop> {
        let b = self.b;
        let start = self.pos;
        let mut i = start;
        while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'$') {
            i += 1;
        }
        let w = &self.s[start..i];
        if matches!(w, "true" | "false" | "null") {
            self.pos = i;
            return Ok(());
        }
        let shown: String = w.chars().take(24).collect();
        Err(inv(start, format!("unexpected word '{shown}' (JSON literals are true, false and null; strings and keys need double quotes)")))
    }

    fn comment(&mut self, start: usize, nls: u8) -> Result<Tok, Stop> {
        let b = self.b;
        let kind = match b.get(start + 1) {
            Some(b'/') => K::LineComment,
            Some(b'*') => K::BlockComment,
            _ => return Err(inv(start, "unexpected character '/'")),
        };
        if !self.jsonc {
            return Err(inv(start, "comments are not allowed in JSON (only in JSONC)"));
        }
        let end = if kind == K::LineComment {
            let stop = b[start..].iter().position(|&c| c == b'\n').map_or(b.len(), |p| start + p);
            self.pos = stop;
            let mut e = stop;
            while e > start + 2 && matches!(b[e - 1], b' ' | b'\t' | b'\r') {
                e -= 1;
            }
            e
        } else {
            match b[start + 2..].windows(2).position(|w| w == b"*/") {
                Some(p) => {
                    self.pos = start + 2 + p + 2;
                    self.pos
                }
                None => return Err(inv(start, "unterminated block comment (missing '*/')")),
            }
        };
        Ok(Tok { kind, start: start as u32, end: end as u32, nl_before: nls > 0, blank_before: nls > 1 })
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Parser (Grammatik-Prüfung mit explizitem Stapel)

#[derive(Clone, Copy, PartialEq, Eq)]
enum St {
    /// Wert erwartet (Anfang, nach `:`).
    Value,
    /// Nach `[`.
    FirstElem,
    /// Nach `,` im Array.
    Elem,
    /// Nach `{`.
    FirstKey,
    /// Nach `,` im Objekt.
    Key,
    /// Nach einem Schlüssel.
    Colon,
    /// Nach einem Wert im Container.
    After,
    /// Nach dem obersten Wert.
    End,
}

fn expected(st: St, obj: Option<bool>, jsonc: bool) -> &'static str {
    match st {
        St::Value => "expected a value",
        St::FirstElem => "expected a value or ']'",
        St::Elem if jsonc => "expected a value or ']'",
        St::Elem => "expected a value after ','",
        St::FirstKey => "expected a string key or '}'",
        St::Key if jsonc => "expected a string key or '}'",
        St::Key => "expected a string key after ','",
        St::Colon => "expected ':' after object key",
        St::After if obj == Some(true) => "expected ',' or '}' after object member",
        St::After => "expected ',' or ']' after array element",
        St::End => "expected end of input after the top-level value",
    }
}

/// Prüft `s[base..]` und reicht jedes Token (auch Kommentare) an `sink` weiter.
fn parse(s: &str, base: usize, jsonc: bool, tick: &mut Ticker, sink: &mut dyn FnMut(Tok)) -> Result<(), Stop> {
    let mut lx = Lexer { s, b: s.as_bytes(), pos: base, jsonc };
    // (Objekt?, Byte-Position der öffnenden Klammer)
    let mut stack: Vec<(bool, u32)> = Vec::new();
    let mut st = St::Value;
    let mut comma_at = 0u32;
    let mut any = false;
    loop {
        tick.at(lx.pos)?;
        let Some(t) = lx.token()? else {
            if st == St::End {
                return Ok(());
            }
            let msg = if !any {
                "the document contains no JSON value".to_string()
            } else {
                let what = expected(st, stack.last().map(|e| e.0), jsonc);
                match stack.last() {
                    Some(&(obj, at)) => {
                        let (l, c) = line_col(s, at as usize);
                        let name = if obj { "object" } else { "array" };
                        format!("{what}, found end of input (the {name} opened at line {l}, column {c} is not closed)")
                    }
                    None => format!("{what}, found end of input"),
                }
            };
            return Err(inv(s.len(), msg));
        };
        if t.kind.is_comment() {
            sink(t);
            continue;
        }
        any = true;
        let obj = stack.last().map(|e| e.0);
        let ok = match (st, t.kind) {
            (St::Value | St::FirstElem | St::Elem, K::ObjOpen) => {
                stack.push((true, t.start));
                st = St::FirstKey;
                true
            }
            (St::Value | St::FirstElem | St::Elem, K::ArrOpen) => {
                stack.push((false, t.start));
                st = St::FirstElem;
                true
            }
            (St::Value | St::FirstElem | St::Elem, K::Str | K::Num | K::Lit) => {
                st = if stack.is_empty() { St::End } else { St::After };
                true
            }
            (St::Elem, K::ArrClose) | (St::Key, K::ObjClose) if !jsonc => {
                return Err(inv(comma_at as usize, "trailing comma is not allowed in JSON (only in JSONC)"));
            }
            (St::FirstElem | St::Elem, K::ArrClose) | (St::FirstKey | St::Key, K::ObjClose) => {
                stack.pop();
                st = if stack.is_empty() { St::End } else { St::After };
                true
            }
            (St::FirstKey | St::Key, K::Str) => {
                st = St::Colon;
                true
            }
            (St::Colon, K::Colon) => {
                st = St::Value;
                true
            }
            (St::After, K::Comma) => {
                comma_at = t.start;
                st = if obj == Some(true) { St::Key } else { St::Elem };
                true
            }
            (St::After, K::ObjClose) if obj == Some(true) => {
                stack.pop();
                st = if stack.is_empty() { St::End } else { St::After };
                true
            }
            (St::After, K::ArrClose) if obj == Some(false) => {
                stack.pop();
                st = if stack.is_empty() { St::End } else { St::After };
                true
            }
            _ => false,
        };
        if !ok {
            return Err(inv(t.start as usize, format!("{}, found {}", expected(st, obj, jsonc), describe(s, t))));
        }
        sink(t);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Ausgabe

/// Trenner vor dem nächsten Token (geordnet: der stärkere gewinnt).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Sep {
    Tight,
    Space,
    Newline,
}

struct Emitter<'a> {
    src: &'a str,
    out: String,
    /// Länge des BOM-Präfixes in `out` (davor wird nie ein Trenner geschrieben).
    base: usize,
    minify: bool,
    /// Eine Leerzeile aus der Eingabe vor einem Eintrag/Kommentar beibehalten (Formatieren, nicht Sortieren).
    keep_blank: bool,
    unit: String,
    cache: String,
    depth: usize,
    /// Trenner, den die Grammatik vor dem nächsten Wert/Schlüssel/Container will.
    sep: Sep,
    /// Gepufferte Kommentare (werden nach `,`/`:` bzw. vor dem nächsten Token geschrieben).
    pending: Vec<Tok>,
    /// Nach einem Zeilenkommentar muss eine neue Zeile folgen.
    force_nl: bool,
    after_comment: bool,
    after_block: bool,
    /// Gerade `{`/`[` geschrieben, noch nichts danach.
    empty_open: bool,
    /// Minify: Komma steht aus (fällt vor `}`/`]` weg).
    comma: bool,
}

impl<'a> Emitter<'a> {
    fn new(src: &'a str, base: usize, minify: bool, indent: Indent, cap: usize) -> Self {
        let mut out = String::with_capacity(cap + cap / 4 + 16);
        out.push_str(&src[..base]);
        let unit = match indent {
            Indent::Tab => "\t".to_string(),
            Indent::Spaces(n) => " ".repeat(n as usize),
        };
        Emitter {
            src,
            out,
            base,
            minify,
            keep_blank: !minify,
            unit,
            cache: String::new(),
            depth: 0,
            sep: Sep::Tight,
            pending: Vec::new(),
            force_nl: false,
            after_comment: false,
            after_block: false,
            empty_open: false,
            comma: false,
        }
    }

    fn text(&self, t: Tok) -> &'a str {
        match t.kind {
            K::Comma => ",",
            K::Colon => ":",
            _ => &self.src[t.start as usize..t.end as usize],
        }
    }

    /// Trenner schreiben; `blank`: vor der neuen Zeile eine Leerzeile (nicht direkt nach `{`/`[`).
    fn put(&mut self, sep: Sep, blank: bool) {
        if self.out.len() == self.base {
            return;
        }
        match sep {
            Sep::Tight => {}
            Sep::Space => self.out.push(' '),
            Sep::Newline => {
                if blank && self.keep_blank && !self.empty_open {
                    self.out.push('\n');
                }
                self.out.push('\n');
                let n = self.depth * self.unit.len();
                while self.cache.len() < n {
                    self.cache.push_str(&self.unit);
                }
                self.out.push_str(&self.cache[..n]);
            }
        }
    }

    fn write(&mut self, t: Tok, sep: Sep) {
        let mut s = sep;
        if self.after_comment && s < Sep::Space {
            s = Sep::Space;
        }
        if self.force_nl || (self.after_block && t.nl_before) {
            s = Sep::Newline;
        }
        self.put(s, t.blank_before && !matches!(t.kind, K::ObjClose | K::ArrClose));
        let txt = self.text(t);
        self.out.push_str(txt);
        self.force_nl = false;
        self.after_comment = false;
        self.after_block = false;
        self.empty_open = false;
    }

    fn flush(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let mut pending = std::mem::take(&mut self.pending);
        for &c in &pending {
            let s = if c.nl_before || self.force_nl { Sep::Newline } else { Sep::Space };
            self.put(s, c.blank_before);
            let txt = self.text(c);
            if txt.contains('\r') {
                self.out.push_str(&txt.replace("\r\n", "\n"));
            } else {
                self.out.push_str(txt);
            }
            let line = c.kind == K::LineComment;
            self.force_nl = line;
            self.after_block = !line;
            self.after_comment = true;
            self.empty_open = false;
        }
        pending.clear();
        self.pending = pending;
    }

    fn push(&mut self, t: Tok) {
        if self.minify {
            match t.kind {
                K::LineComment | K::BlockComment => {}
                K::Comma => self.comma = true,
                K::ObjClose | K::ArrClose => {
                    self.comma = false;
                    let x = self.text(t);
                    self.out.push_str(x);
                }
                _ => {
                    if self.comma {
                        self.out.push(',');
                        self.comma = false;
                    }
                    let x = self.text(t);
                    self.out.push_str(x);
                }
            }
            return;
        }
        match t.kind {
            K::LineComment | K::BlockComment => self.pending.push(t),
            K::Comma => {
                self.write(t, Sep::Tight);
                self.sep = Sep::Newline;
                self.flush();
            }
            K::Colon => {
                self.write(t, Sep::Tight);
                self.sep = Sep::Space;
                self.flush();
            }
            K::ObjOpen | K::ArrOpen => {
                self.flush();
                let s = self.sep;
                self.write(t, s);
                self.depth += 1;
                self.sep = Sep::Newline;
                self.empty_open = true;
            }
            K::ObjClose | K::ArrClose => {
                self.flush();
                self.depth = self.depth.saturating_sub(1);
                let s = if self.empty_open { Sep::Tight } else { Sep::Newline };
                self.write(t, s);
                self.sep = Sep::Tight;
            }
            K::Str | K::Num | K::Lit => {
                self.flush();
                let s = self.sep;
                self.write(t, s);
                self.sep = Sep::Tight;
            }
        }
    }

    fn finish(mut self, final_newline: bool) -> String {
        self.flush();
        if final_newline {
            self.out.push('\n');
        }
        self.out
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Schlüssel sortieren

/// Ein Container (`{…}`/`[…]`) als Token-Bereiche.
struct Cont {
    open: u32,
    close: u32,
    /// Kommentare direkt hinter der öffnenden Klammer in derselben Zeile: `open+1..head_end`.
    head_end: u32,
    /// Bereich in der Eintragsliste.
    items: (u32, u32),
    /// Kommentare vor der schließenden Klammer, die zu keinem Eintrag gehören: `dangling..close`.
    dangling: u32,
    trailing_comma: bool,
    obj: bool,
}

/// Ein Eintrag: Kommentare davor, Schlüssel, `:`, Wert, Kommentare vor dem Komma – ohne das Komma selbst.
/// `trail`: Kommentare hinter dem Komma in derselben Zeile.
#[derive(Clone, Copy)]
struct Item {
    start: u32,
    end: u32,
    trail: (u32, u32),
    key: u32,
}

struct Frame {
    slot: usize,
    items: Vec<Item>,
    head: bool,
    after_comma: bool,
    cur_start: Option<u32>,
    key: u32,
    has_value: bool,
    last_sig_end: u32,
}

impl Frame {
    fn sig(&mut self, i: u32) {
        self.head = false;
        self.after_comma = false;
        if self.cur_start.is_none() {
            self.cur_start = Some(i);
        }
        if !self.has_value {
            self.key = i;
        }
        self.has_value = true;
        self.last_sig_end = i + 1;
    }
}

enum Task {
    Toks(u32, u32),
    Cont(usize),
    Tok(u32),
    Comma,
}

fn sort_keys(s: &str, base: usize, jsonc: bool, opts: Options, tick: &mut Ticker) -> Result<String, Stop> {
    let mut toks: Vec<Tok> = Vec::with_capacity(s.len() / 8);
    tick.phase(0.0, 0.5, s.len());
    parse(s, base, jsonc, tick, &mut |t| toks.push(t))?;
    let n = toks.len();

    // Aufbau: Container und Einträge (iterativ).
    tick.phase(0.5, 0.6, n);
    let mut conts: Vec<Cont> = Vec::new();
    let mut items: Vec<Item> = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();
    for i in 0..n {
        tick.at(i)?;
        let t = toks[i];
        let iu = i as u32;
        match t.kind {
            K::LineComment | K::BlockComment => {
                if let Some(f) = stack.last_mut() {
                    if f.head && !t.nl_before {
                        conts[f.slot].head_end = iu + 1;
                    } else if f.after_comma && !t.nl_before && !f.items.is_empty() {
                        if let Some(it) = f.items.last_mut() {
                            it.trail.1 = iu + 1;
                        }
                    } else {
                        f.head = false;
                        f.after_comma = false;
                        f.cur_start.get_or_insert(iu);
                    }
                }
            }
            K::Comma => {
                if let Some(f) = stack.last_mut() {
                    f.items.push(Item { start: f.cur_start.unwrap_or(iu), end: iu, trail: (iu + 1, iu + 1), key: f.key });
                    f.after_comma = true;
                    f.head = false;
                    f.cur_start = None;
                    f.has_value = false;
                }
            }
            K::ObjOpen | K::ArrOpen => {
                if let Some(f) = stack.last_mut() {
                    f.sig(iu);
                }
                conts.push(Cont {
                    open: iu,
                    close: iu,
                    head_end: iu + 1,
                    items: (0, 0),
                    dangling: iu,
                    trailing_comma: false,
                    obj: t.kind == K::ObjOpen,
                });
                stack.push(Frame {
                    slot: conts.len() - 1,
                    items: Vec::new(),
                    head: true,
                    after_comma: false,
                    cur_start: None,
                    key: 0,
                    has_value: false,
                    last_sig_end: iu + 1,
                });
            }
            K::ObjClose | K::ArrClose => {
                let Some(mut f) = stack.pop() else { continue };
                let (dangling, trailing) = if f.has_value {
                    // Letzter Eintrag ohne Komma: Kommentare in derselben Zeile gehören noch dazu.
                    let mut e = f.last_sig_end;
                    while e < iu && toks[e as usize].kind.is_comment() && !toks[e as usize].nl_before {
                        e += 1;
                    }
                    f.items.push(Item { start: f.cur_start.unwrap_or(e), end: e, trail: (e, e), key: f.key });
                    (e, false)
                } else {
                    (f.cur_start.unwrap_or(iu), !f.items.is_empty())
                };
                let a = items.len() as u32;
                items.extend_from_slice(&f.items);
                let c = &mut conts[f.slot];
                c.close = iu;
                c.items = (a, items.len() as u32);
                c.dangling = dangling;
                c.trailing_comma = trailing;
                if let Some(p) = stack.last_mut() {
                    p.last_sig_end = iu + 1;
                }
            }
            K::Colon | K::Str | K::Num | K::Lit => {
                if let Some(f) = stack.last_mut() {
                    f.sig(iu);
                }
            }
        }
    }

    // Ausgabe in sortierter Reihenfolge (expliziter Arbeitsstapel statt Rekursion).
    tick.phase(0.6, 1.0, n);
    let mut em = Emitter::new(s, base, false, opts.indent, s.len());
    // Leerzeilen passen nach dem Umstellen nicht mehr zu ihren Abschnitten.
    em.keep_blank = false;
    let comma = Tok { kind: K::Comma, start: 0, end: 0, nl_before: false, blank_before: false };
    let mut work = vec![Task::Toks(0, n as u32)];
    let mut done = 0usize;
    while let Some(task) = work.pop() {
        match task {
            Task::Toks(a, b) => {
                let mut i = a;
                while i < b {
                    let t = toks[i as usize];
                    if t.kind.is_open()
                        && let Ok(ci) = conts.binary_search_by_key(&i, |c| c.open)
                    {
                        let close = conts[ci].close;
                        if close + 1 < b {
                            work.push(Task::Toks(close + 1, b));
                        }
                        work.push(Task::Cont(ci));
                        break;
                    }
                    em.push(t);
                    done += 1;
                    tick.at(done)?;
                    i += 1;
                }
            }
            Task::Cont(ci) => {
                let c = &conts[ci];
                for j in c.open..c.head_end {
                    em.push(toks[j as usize]);
                    done += 1;
                }
                work.push(Task::Tok(c.close));
                work.push(Task::Toks(c.dangling, c.close));
                let its = &items[c.items.0 as usize..c.items.1 as usize];
                let mut order: Vec<usize> = (0..its.len()).collect();
                if c.obj && its.len() > 1 {
                    // Stabil: gleiche Schlüssel behalten ihre Reihenfolge.
                    order.sort_by_cached_key(|&k| key_text(s, toks[its[k].key as usize]));
                }
                let m = order.len();
                for (p, &k) in order.iter().enumerate().rev() {
                    let it = its[k];
                    work.push(Task::Toks(it.trail.0, it.trail.1));
                    if p + 1 < m || c.trailing_comma {
                        work.push(Task::Comma);
                    }
                    work.push(Task::Toks(it.start, it.end));
                }
                tick.at(done)?;
            }
            Task::Tok(i) => {
                em.push(toks[i as usize]);
                done += 1;
            }
            Task::Comma => em.push(comma),
        }
    }
    Ok(em.finish(opts.final_newline))
}

/// Schlüssel zum Vergleichen (ohne Anführungszeichen, Escapes dekodiert).
fn key_text(s: &str, t: Tok) -> Cow<'_, str> {
    let raw = s.get(t.start as usize..t.end as usize).unwrap_or("");
    let inner = raw.strip_prefix('"').and_then(|r| r.strip_suffix('"')).unwrap_or(raw);
    if inner.contains('\\') {
        Cow::Owned(unescape(inner))
    } else {
        Cow::Borrowed(inner)
    }
}

fn hex4(s: &str) -> Option<u32> {
    let h = s.get(..4)?;
    if h.bytes().all(|c| c.is_ascii_hexdigit()) {
        u32::from_str_radix(h, 16).ok()
    } else {
        None
    }
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('\\') {
        out.push_str(&rest[..p]);
        rest = &rest[p + 1..];
        let mut chars = rest.chars();
        let Some(c) = chars.next() else { break };
        rest = chars.as_str();
        match c {
            'b' => out.push('\u{8}'),
            'f' => out.push('\u{c}'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'u' => {
                let Some(hi) = hex4(rest) else {
                    out.push('\u{fffd}');
                    continue;
                };
                rest = &rest[4..];
                if (0xD800..0xDC00).contains(&hi)
                    && let Some(lo) = rest.strip_prefix("\\u").and_then(hex4).filter(|lo| (0xDC00..0xE000).contains(lo))
                {
                    rest = &rest[6..];
                    out.push(char::from_u32(0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)).unwrap_or('\u{fffd}'));
                    continue;
                }
                out.push(char::from_u32(hi).unwrap_or('\u{fffd}'));
            }
            other => out.push(other),
        }
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::time::Instant;

    fn opts() -> Options {
        Options::default()
    }

    fn go_with(jsonc: bool, op: Op, s: &str, o: Options) -> Outcome {
        run(jsonc, op, s, o, &AtomicBool::new(false), &|_| {})
    }

    fn ok_with(jsonc: bool, op: Op, s: &str, o: Options) -> String {
        match go_with(jsonc, op, s, o) {
            Outcome::Text(t) => t,
            other => panic!("{op:?} failed on {s:?}: {other:?}"),
        }
    }

    fn ok(jsonc: bool, op: Op, s: &str) -> String {
        ok_with(jsonc, op, s, opts())
    }

    fn fmt(s: &str) -> String {
        ok(false, Op::Format, s)
    }

    fn fmtc(s: &str) -> String {
        ok(true, Op::Format, s)
    }

    fn err(jsonc: bool, s: &str) -> FormatError {
        match go_with(jsonc, Op::Validate, s, opts()) {
            Outcome::Invalid(e) => {
                // Formatieren, Minifizieren und Sortieren melden denselben Fehler.
                for op in [Op::Format, Op::Minify, Op::SortKeys] {
                    assert_eq!(go_with(jsonc, op, s, opts()), Outcome::Invalid(e.clone()), "{op:?} on {s:?}");
                }
                e
            }
            other => panic!("expected an error for {s:?}, got {other:?}"),
        }
    }

    #[test]
    fn format_nested() {
        let s = r#"{"a":1,"b":[true,false,null],"c":{"d":"x","e":[[]]}}"#;
        let want = "{\n  \"a\": 1,\n  \"b\": [\n    true,\n    false,\n    null\n  ],\n  \"c\": {\n    \"d\": \"x\",\n    \"e\": [\n      []\n    ]\n  }\n}\n";
        assert_eq!(fmt(s), want);
        assert_eq!(fmt(want), want);
    }

    #[test]
    fn format_empty_containers() {
        assert_eq!(fmt(" { } "), "{}\n");
        assert_eq!(fmt("[\n\n]"), "[]\n");
        assert_eq!(fmt(r#"{"a":{ },"b":[ ],"c":[{}]}"#), "{\n  \"a\": {},\n  \"b\": [],\n  \"c\": [\n    {}\n  ]\n}\n");
    }

    #[test]
    fn format_scalars_unicode_escapes_numbers() {
        assert_eq!(fmt("  \"abc\"  "), "\"abc\"\n");
        assert_eq!(fmt("null"), "null\n");
        let s = r#"{"ä":"日本\u00e9\n\"x\"\/","emoji":"😀","\uD83D\uDE00":"\\"}"#;
        assert_eq!(fmt(s), "{\n  \"ä\": \"日本\\u00e9\\n\\\"x\\\"\\/\",\n  \"emoji\": \"😀\",\n  \"\\uD83D\\uDE00\": \"\\\\\"\n}\n");
        let n = "[1.0,1e400,0.1000,-0,1E+2,-2.5e-10,123456789012345678901234567890,0]";
        assert_eq!(ok(false, Op::Minify, n), format!("{n}\n"));
        assert_eq!(fmt(n), "[\n  1.0,\n  1e400,\n  0.1000,\n  -0,\n  1E+2,\n  -2.5e-10,\n  123456789012345678901234567890,\n  0\n]\n");
    }

    #[test]
    fn format_duplicate_keys_kept() {
        assert_eq!(ok(false, Op::Minify, r#"{ "a": 1, "a": 2 }"#), "{\"a\":1,\"a\":2}\n");
    }

    #[test]
    fn format_bom_indent_final_newline() {
        assert_eq!(fmt("\u{feff}{\"a\":1}"), "\u{feff}{\n  \"a\": 1\n}\n");
        assert_eq!(ok(false, Op::Minify, "\u{feff} [ 1 ] "), "\u{feff}[1]\n");
        assert_eq!(ok(false, Op::Validate, "\u{feff} [ 1 ] "), "\u{feff} [ 1 ] ");
        let tab = Options { indent: Indent::Tab, final_newline: false };
        assert_eq!(ok_with(false, Op::Format, r#"{"a":[1]}"#, tab), "{\n\t\"a\": [\n\t\t1\n\t]\n}");
        let four = Options { indent: Indent::Spaces(4), final_newline: true };
        assert_eq!(ok_with(false, Op::Format, r#"{"a":[1]}"#, four), "{\n    \"a\": [\n        1\n    ]\n}\n");
        let no_nl = Options { indent: Indent::Spaces(2), final_newline: false };
        assert_eq!(ok_with(false, Op::Minify, "[1, 2]\n", no_nl), "[1,2]");
    }

    #[test]
    fn format_keeps_one_blank_line() {
        let s = "{\n\n  \"a\": 1,\n\n\n\n  \"b\": [\n\n    1,\n\n    2\n\n  ]\n\n}";
        assert_eq!(fmt(s), "{\n  \"a\": 1,\n\n  \"b\": [\n    1,\n\n    2\n  ]\n}\n");
        let c = "// head\n\n{\n  \"a\": 1, // x\n\n  // section\n  \"b\": 2\n\n  // end\n}";
        let want = "// head\n\n{\n  \"a\": 1, // x\n\n  // section\n  \"b\": 2\n\n  // end\n}\n";
        assert_eq!(fmtc(c), want);
        assert_eq!(fmtc(want), want);
        // Minifizieren und Sortieren lassen Leerzeilen weg.
        assert_eq!(ok(false, Op::Minify, s), "{\"a\":1,\"b\":[1,2]}\n");
        assert_eq!(ok(false, Op::SortKeys, "{\"b\":1,\n\n\"a\":2}"), "{\n  \"a\": 2,\n  \"b\": 1\n}\n");
    }

    #[test]
    fn minify() {
        let s = "{ \"a\" : [ 1 , 2 ] ,\n\t\"b\" : { } , \"c\":\"x y\" }\n";
        assert_eq!(ok(false, Op::Minify, s), "{\"a\":[1,2],\"b\":{},\"c\":\"x y\"}\n");
    }

    #[test]
    fn validate_returns_input_unchanged() {
        let s = "{ \"a\" :1,\n   \"b\":[ ] }";
        assert_eq!(ok(false, Op::Validate, s), s);
        let c = "// x\n{\"a\":1,}";
        assert_eq!(ok(true, Op::Validate, c), c);
    }

    #[test]
    fn validate_positions_and_messages() {
        let e = err(false, r#"{"a":1 "b":2}"#);
        assert_eq!((e.line, e.column), (1, 8));
        assert!(e.message.starts_with("expected ',' or '}' after object member"), "{}", e.message);
        // Mehrbyte-Zeichen vor dem Fehler zählen als je ein Zeichen.
        let e = err(false, "{\n  \"äöü\": \"日本語😀\" x\n}");
        assert_eq!((e.line, e.column), (2, 17), "{}", e.message);
        assert!(e.message.contains("unexpected word 'x'"), "{}", e.message);
        let e = err(false, "{\n\t\"ä\": [1, 2,]\n}");
        assert_eq!((e.line, e.column), (2, 12));
        assert!(e.message.contains("trailing comma"), "{}", e.message);
        let e = err(false, "{\"a\":1 // x\n}");
        assert_eq!((e.line, e.column), (1, 8));
        assert!(e.message.contains("comments are not allowed"), "{}", e.message);
        let e = err(false, "/* x */ 1");
        assert_eq!((e.line, e.column), (1, 1));
        let e = err(false, "[1, 2");
        assert_eq!((e.line, e.column), (1, 6));
        assert!(e.message.contains("array opened at line 1, column 1 is not closed"), "{}", e.message);
        let e = err(false, "{\"a\" 1}");
        assert_eq!((e.line, e.column, e.message.as_str()), (1, 6, "expected ':' after object key, found a number"));
        let e = err(false, "[1}");
        assert_eq!((e.line, e.column, e.message.as_str()), (1, 3, "expected ',' or ']' after array element, found '}'"));
        let e = err(false, "\"ab\ncd\"");
        assert_eq!((e.line, e.column), (1, 4));
        let e = err(false, "\"abc");
        assert_eq!((e.line, e.column), (1, 1));
        let e = err(false, r#"["\x"]"#);
        assert_eq!((e.line, e.column), (1, 3));
        assert!(e.message.contains("invalid escape sequence '\\x'"), "{}", e.message);
        let e = err(false, r#"["\u12G4"]"#);
        assert_eq!((e.line, e.column), (1, 3));
        let e = err(false, "[01]");
        assert_eq!((e.line, e.column), (1, 2));
        assert!(e.message.contains("leading zeros"));
        let e = err(false, "[1.]");
        assert_eq!((e.line, e.column), (1, 4));
        let e = err(false, "[1e]");
        assert_eq!((e.line, e.column), (1, 4));
        let e = err(false, "[-]");
        assert_eq!((e.line, e.column), (1, 3));
        let e = err(false, "   \n  ");
        assert_eq!((e.line, e.column), (2, 3));
        assert!(e.message.contains("no JSON value"));
        let e = err(true, "// only a comment\n");
        assert!(e.message.contains("no JSON value"));
        let e = err(false, "{} {}");
        assert_eq!((e.line, e.column), (1, 4));
        assert!(e.message.starts_with("expected end of input"));
        let e = err(false, "{'a':1}");
        assert_eq!((e.line, e.column), (1, 2));
        assert!(e.message.contains("double quotes"));
        let e = err(false, "[True]");
        assert!(e.message.contains("'True'"));
        let e = err(false, "[1,\u{a0}2]");
        assert_eq!((e.line, e.column), (1, 4));
        assert!(e.message.contains("U+00A0"), "{}", e.message);
        let e = err(true, "[1 /* open");
        assert_eq!((e.line, e.column), (1, 4));
        assert!(e.message.contains("unterminated block comment"));
        let e = err(true, "[1,,2]");
        assert_eq!((e.line, e.column), (1, 4));
        let e = err(true, "{,}");
        assert_eq!((e.line, e.column), (1, 2));
        let e = err(false, "\u{feff}[x]");
        assert_eq!((e.line, e.column), (1, 3));
        let e = err(false, "[\"tab\there\"]");
        assert_eq!((e.line, e.column), (1, 6));
        assert!(e.message.contains("control character U+0009"));
    }

    #[test]
    fn sort_keys_recursive_and_stable() {
        let s = r#"{"b":1,"a":{"d":[{"z":1,"y":2}],"c":2},"a":0,"B":3}"#;
        let want = "{\n  \"B\": 3,\n  \"a\": {\n    \"c\": 2,\n    \"d\": [\n      {\n        \"y\": 2,\n        \"z\": 1\n      }\n    ]\n  },\n  \"a\": 0,\n  \"b\": 1\n}\n";
        let got = ok(false, Op::SortKeys, s);
        assert_eq!(got, want);
        assert_eq!(ok(false, Op::SortKeys, &got), got);
        // Escapes werden zum Vergleichen dekodiert, aber unverändert ausgegeben.
        assert_eq!(ok(false, Op::SortKeys, r#"{"b":1,"\u0061":2}"#), "{\n  \"\\u0061\": 2,\n  \"b\": 1\n}\n");
        // Array-Reihenfolge bleibt, Skalare und leere Container gehen.
        assert_eq!(ok(false, Op::SortKeys, "[3,1,{}]"), "[\n  3,\n  1,\n  {}\n]\n");
        assert_eq!(ok(false, Op::SortKeys, "\u{feff}7"), "\u{feff}7\n");
    }

    #[test]
    fn unescape_keys() {
        assert_eq!(unescape(r#"a\u00e9\n\"\\\/\uD83D\uDE00\uD800x"#), "aé\n\"\\/😀\u{fffd}x");
    }

    const JSONC: &str = "// head\n{\n  // leading a\n  \"a\": 1, // trailing a\n  /* block */ \"b\": [ // after open\n    1, 2, // after 2\n  ],\n  \"c\": /* inline */ 3,\n  \"d\": {}, // empty\n  \"e\": [ /* inside empty */ ],\n  \"f\": 4 // last\n  // dangling\n}\n// tail\n";

    #[test]
    fn jsonc_format_keeps_comments_and_trailing_commas() {
        let want = "// head\n{\n  // leading a\n  \"a\": 1, // trailing a\n  /* block */\n  \"b\": [ // after open\n    1,\n    2, // after 2\n  ],\n  \"c\": /* inline */ 3,\n  \"d\": {}, // empty\n  \"e\": [ /* inside empty */\n  ],\n  \"f\": 4 // last\n  // dangling\n}\n// tail\n";
        let got = fmtc(JSONC);
        assert_eq!(got, want);
        assert_eq!(fmtc(&got), got);
        assert!(has_comments(JSONC));
    }

    #[test]
    fn jsonc_comments_in_odd_places() {
        // Kommentar vor dem Komma wandert hinter das Komma.
        assert_eq!(fmtc("[1 // c\n, 2]"), "[\n  1, // c\n  2\n]\n");
        assert_eq!(fmtc("[1 /* c */, 2]"), "[\n  1, /* c */\n  2\n]\n");
        // Zwischen Schlüssel und Doppelpunkt, nach dem Doppelpunkt.
        assert_eq!(fmtc("{\"a\" /* k */ : // v\n 1}"), "{\n  \"a\": /* k */ // v\n  1\n}\n");
        // Leere Container mit Kommentar.
        assert_eq!(fmtc("{ // c\n}"), "{ // c\n}\n");
        assert_eq!(fmtc("[\n// c\n]"), "[\n  // c\n]\n");
        // Ganz oben, ganz unten, Blockkommentar vor dem Wert in derselben Zeile.
        assert_eq!(fmtc("/* a */ 1 /* b */"), "/* a */ 1 /* b */\n");
        assert_eq!(fmtc("/* a */\n1"), "/* a */\n1\n");
        assert_eq!(fmtc("\u{feff}// x\n[]"), "\u{feff}// x\n[]\n");
        // Mehrzeiliger Blockkommentar bleibt wörtlich, CRLF darin wird zu LF; Zeilenkommentar ohne Leerraum am Ende.
        assert_eq!(fmtc("[1, /* x\r\n   y */ 2 // z  \r\n]"), "[\n  1, /* x\n   y */\n  2 // z\n]\n");
        // Komma am Ende in allen Lagen.
        let tc = "{\"a\":[1,2,],\"b\":{\"c\":1,},}";
        assert_eq!(fmtc(tc), "{\n  \"a\": [\n    1,\n    2,\n  ],\n  \"b\": {\n    \"c\": 1,\n  },\n}\n");
        assert_eq!(ok(true, Op::Minify, tc), "{\"a\":[1,2],\"b\":{\"c\":1}}\n");
        assert!(err(false, tc).message.contains("trailing comma"));
        for s in ["[1 // c\n, 2]", "{\"a\" /* k */ : // v\n 1}", "/* a */ 1 /* b */", tc, "[1, /* x\r\n   y */ 2 // z  \r\n]"] {
            let once = fmtc(s);
            assert_eq!(fmtc(&once), once, "idempotent: {s:?}");
        }
    }

    #[test]
    fn jsonc_minify_removes_comments() {
        assert_eq!(ok(true, Op::Minify, JSONC), "{\"a\":1,\"b\":[1,2],\"c\":3,\"d\":{},\"e\":[],\"f\":4}\n");
    }

    #[test]
    fn jsonc_sort_keys_moves_comments_with_members() {
        let s = "{\n  // about b\n  \"b\": 1, // tb\n  // about a\n  \"a\": 2 // ta\n}";
        let want = "{\n  // about a\n  \"a\": 2, // ta\n  // about b\n  \"b\": 1 // tb\n}\n";
        assert_eq!(ok(true, Op::SortKeys, s), want);
        let s = "{ // head\n  \"b\": [ // x\n    {\"d\":1,\"c\":2,}, ],\n  \"a\": 2,\n  // dangling\n}";
        let want = "{ // head\n  \"a\": 2,\n  \"b\": [ // x\n    {\n      \"c\": 2,\n      \"d\": 1,\n    },\n  ],\n  // dangling\n}\n";
        assert_eq!(ok(true, Op::SortKeys, s), want);
        let sorted = ok(true, Op::SortKeys, JSONC);
        assert_eq!(ok(true, Op::SortKeys, &sorted), sorted);
        assert_eq!(ok(true, Op::Minify, &sorted), ok(true, Op::Minify, JSONC));
    }

    #[test]
    fn has_comments_ignores_strings() {
        assert!(!has_comments(r#"{"url":"http://example.org/*x*/"}"#));
        assert!(!has_comments(r#"{"a":"\"//"}"#));
        assert!(has_comments("{\"a\":1}//x"));
        assert!(has_comments("/* */{}"));
        assert!(has_comments("{\"a\":\"broken\n// c\n}"));
        assert!(!has_comments("{\"a\": 1 / 2}"));
        assert!(!has_comments(""));
    }

    #[test]
    fn cancel_before_and_during() {
        let big = big_doc(2 << 20);
        let cancel = AtomicBool::new(true);
        for op in [Op::Format, Op::Minify, Op::Validate, Op::SortKeys] {
            assert_eq!(run(false, op, &big, opts(), &cancel, &|_| {}), Outcome::Cancelled);
        }
        for op in [Op::Format, Op::Minify, Op::Validate, Op::SortKeys] {
            let cancel = AtomicBool::new(false);
            let seen = RefCell::new(0.0f32);
            let out = run(false, op, &big, opts(), &cancel, &|f| {
                *seen.borrow_mut() = f;
                if f > 0.3 {
                    cancel.store(true, Ordering::Relaxed);
                }
            });
            assert_eq!(out, Outcome::Cancelled, "{op:?}");
            assert!(*seen.borrow() < 0.5, "{op:?} stopped late: {}", seen.borrow());
        }
    }

    #[test]
    fn progress_is_monotonic() {
        let big = big_doc(1 << 20);
        for op in [Op::Format, Op::Minify, Op::Validate, Op::SortKeys] {
            let seen = RefCell::new(Vec::new());
            let out = run(false, op, &big, opts(), &AtomicBool::new(false), &|f| seen.borrow_mut().push(f));
            assert!(matches!(out, Outcome::Text(_)));
            let v = seen.into_inner();
            assert!(v.len() >= 10 && v.len() <= 400, "{op:?}: {} reports", v.len());
            assert!(v.windows(2).all(|w| w[0] <= w[1]), "{op:?} not monotonic");
            assert!(v.iter().all(|f| (0.0..=1.0).contains(f)));
            assert_eq!(v.first(), Some(&0.0));
            assert_eq!(v.last(), Some(&1.0));
        }
    }

    /// Ein gültiges Dokument von ungefähr `size` Bytes (Objekte, Arrays, Zahlen, Zeichenketten mit Escapes).
    fn big_doc(size: usize) -> String {
        let mut s = String::with_capacity(size + 256);
        s.push('[');
        let mut i = 0u64;
        while s.len() < size {
            if i > 0 {
                s.push(',');
            }
            s.push_str(&format!(
                "{{\"zid\":{i},\"name\":\"Eintrag {i} äöü \\u00e9 \\\"q\\\"\",\"value\":{}.{:04},\"tags\":[\"a\",\"b\",null,true],\"nested\":{{\"y\":[1,2,{{}}],\"x\":-1e-7}}}}",
                i * 7,
                i % 10000
            ));
            i += 1;
        }
        s.push(']');
        s
    }

    #[test]
    #[cfg_attr(debug_assertions, ignore = "slow in debug builds; runs with cargo test --release")]
    fn performance_50mb() {
        let doc = big_doc(50 << 20);
        let mb = doc.len() as f64 / (1 << 20) as f64;
        let mut report = Vec::new();
        for op in [Op::Validate, Op::Format, Op::Minify, Op::SortKeys] {
            let t = Instant::now();
            let out = run(false, op, &doc, opts(), &AtomicBool::new(false), &|_| {});
            let dt = t.elapsed();
            let Outcome::Text(text) = out else { panic!("{op:?} failed") };
            report.push(format!("{op:?} {:.2}s ({} MB out)", dt.as_secs_f64(), text.len() >> 20));
            // Großzügige Grenze: der Test läuft parallel zu über 3000 anderen (allein: 0,2–1,4 s je Befehl).
            assert!(dt.as_secs_f64() < 30.0, "{op:?} took {dt:?} for {mb:.0} MB");
        }
        let t = Instant::now();
        let comments = has_comments(&doc);
        report.push(format!("has_comments {:.2}s", t.elapsed().as_secs_f64()));
        assert!(!comments);
        eprintln!("json 50 MB ({mb:.1} MB): {}", report.join(", "));
    }

    #[test]
    fn deep_nesting_without_recursion() {
        const N: usize = 10_000;
        let flat = Options { indent: Indent::Spaces(0), final_newline: false };
        let arr = format!("{}{}", "[".repeat(N), "]".repeat(N));
        let obj = format!("{}1{}", "{\"a\":".repeat(N), "}".repeat(N));
        let cmt = format!("{}{}", "[ // c\n".repeat(N), "]".repeat(N));
        for (jsonc, doc) in [(false, &arr), (false, &obj), (true, &cmt)] {
            for op in [Op::Validate, Op::Minify, Op::Format, Op::SortKeys] {
                let out = ok_with(jsonc, op, doc, flat);
                if op == Op::Minify && !jsonc {
                    assert_eq!(&out, doc);
                }
            }
        }
        // Echte Einrückung bei voller Tiefe.
        let tab = Options { indent: Indent::Tab, final_newline: true };
        let out = ok_with(false, Op::Format, &arr, tab);
        assert_eq!(out.lines().count(), 2 * N - 1);
        assert!(out.lines().nth(N - 1).is_some_and(|l| l == format!("{}[]", "\t".repeat(N - 1))));
        // Nicht geschlossen bei voller Tiefe.
        let e = err(false, &"[".repeat(N));
        assert_eq!((e.line, e.column), (1, N + 1));
    }

    /// Kleiner deterministischer Zufallsgenerator (xorshift64*).
    struct Rng(u64);
    impl Rng {
        fn next_u64(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next_u64() % n as u64) as usize
        }
    }

    const PIECES: &[&str] = &[
        "{", "}", "[", "]", ":", ",", " ", "\n", "\"a\"", "\"b\"", "\"\\n\"", "\"\\u00e9\"", "\"é😀\"", "0", "-1", "2.5", "1e3",
        "true", "false", "null", "// c\n", "/* d */", "\t", "\"", "\\", "x", "/", "*", "\r\n",
    ];

    const ALPHA: &[u8] = b"{}[]:,\"\\/* \n0123456789.-+eEtrufalsn";

    fn soup(r: &mut Rng, len: usize) -> String {
        (0..len).map(|_| PIECES[r.below(PIECES.len())]).collect()
    }

    /// Ein zufälliges gültiges (JSONC-)Dokument.
    fn valid_doc(r: &mut Rng, depth: usize, out: &mut String) {
        let ws = |r: &mut Rng, out: &mut String| match r.below(8) {
            0 => out.push_str(" // w\n"),
            1 => out.push_str("/* b */"),
            2 => out.push('\n'),
            3 => out.push(' '),
            _ => {}
        };
        ws(r, out);
        match if depth > 4 { 2 + r.below(4) } else { r.below(6) } {
            0 | 1 => {
                let obj = r.below(2) == 0;
                out.push(if obj { '{' } else { '[' });
                let n = r.below(4);
                for i in 0..n {
                    if i > 0 {
                        out.push(',');
                    }
                    ws(r, out);
                    if obj {
                        out.push_str(["\"k\"", "\"a\"", "\"\\u0062\"", "\"ü\""][r.below(4)]);
                        ws(r, out);
                        out.push(':');
                    }
                    valid_doc(r, depth + 1, out);
                }
                if n > 0 && r.below(3) == 0 {
                    out.push(',');
                }
                ws(r, out);
                out.push(if obj { '}' } else { ']' });
            }
            2 => out.push_str(["0", "-1.50", "1e400", "3E-2"][r.below(4)]),
            3 => out.push_str(["\"s\"", "\"\\\"//\"", "\"日本\""][r.below(3)]),
            _ => out.push_str(["true", "false", "null"][r.below(3)]),
        }
        ws(r, out);
    }

    fn check_all_ops(jsonc: bool, s: &str) {
        for op in [Op::Validate, Op::Format, Op::Minify, Op::SortKeys] {
            let out = go_with(jsonc, op, s, opts());
            if let Outcome::Invalid(e) = &out {
                assert!(e.line >= 1 && e.column >= 1);
                assert!(!e.message.is_empty());
            }
            let Outcome::Text(t) = out else { continue };
            match op {
                Op::Validate => assert_eq!(t, s),
                Op::Format | Op::SortKeys => {
                    assert_eq!(ok(jsonc, op, &t), t, "{op:?} not idempotent on {s:?}");
                    if op == Op::Format {
                        assert_eq!(ok(jsonc, Op::Minify, &t), ok(jsonc, Op::Minify, s), "format changed content of {s:?}");
                    }
                }
                Op::Minify => {
                    assert_eq!(ok(jsonc, Op::Minify, &t), t);
                    // Ohne Kommentare und Komma am Ende ist die Ausgabe immer strenges JSON.
                    assert!(matches!(go_with(false, Op::Validate, &t, opts()), Outcome::Text(_)), "{t:?}");
                }
            }
        }
    }

    #[test]
    fn fuzz_no_panics() {
        let mut r = Rng(0x9E37_79B9_7F4A_7C15);
        for _ in 0..3000 {
            // Zufällige Bytes (überwiegend JSON-Zeichen), verlustbehaftet als UTF-8.
            let len = r.below(40);
            let bytes: Vec<u8> = (0..len)
                .map(|_| if r.below(4) == 0 { r.next_u64() as u8 } else { ALPHA[r.below(ALPHA.len())] })
                .collect();
            let s = String::from_utf8_lossy(&bytes).into_owned();
            check_all_ops(false, &s);
            check_all_ops(true, &s);
            let len = 1 + r.below(30);
            let s = soup(&mut r, len);
            check_all_ops(false, &s);
            check_all_ops(true, &s);
        }
        // Gültige Dokumente und jede Abkürzung davon.
        for _ in 0..300 {
            let mut doc = String::new();
            valid_doc(&mut r, 0, &mut doc);
            check_all_ops(true, &doc);
            assert!(matches!(go_with(true, Op::Validate, &doc, opts()), Outcome::Text(_)), "{doc:?}");
            for (i, _) in doc.char_indices() {
                check_all_ops(true, &doc[..i]);
            }
        }
    }

    #[test]
    fn strict_mode_agrees_with_serde_json() {
        let mut r = Rng(0x1234_5678_9ABC_DEF1);
        let mut valid = 0;
        for _ in 0..20_000 {
            let len = 1 + r.below(14);
            let s = soup(&mut r, len);
            let ours = go_with(false, Op::Validate, &s, opts());
            let theirs = serde_json::from_str::<serde_json::Value>(&s);
            if let Err(e) = &theirs
                && (e.to_string().contains("out of range") || e.to_string().contains("recursion"))
            {
                continue;
            }
            assert_eq!(matches!(ours, Outcome::Text(_)), theirs.is_ok(), "disagree on {s:?}: {ours:?} / {theirs:?}");
            if let Ok(v) = theirs {
                valid += 1;
                for op in [Op::Minify, Op::Format, Op::SortKeys] {
                    let t = ok(false, op, &s);
                    let back: serde_json::Value = serde_json::from_str(&t).unwrap();
                    assert_eq!(back, v, "{op:?} changed {s:?}");
                }
            }
        }
        assert!(valid > 100, "too few valid samples: {valid}");
    }
}
