//! XML behutsam formatieren, minifizieren und auf Wohlgeformtheit prüfen (#381, 3.2.0).
//!
//! Eigener Tokenizer ohne Crate und ohne Netzwerk: externe DTDs und Entitäten werden nie geladen oder aufgelöst,
//! Entitäts- und Zeichenreferenzen bleiben, wie sie geschrieben sind. Zwei Durchläufe, beide ohne Rekursion
//! (10 000 Ebenen Verschachtelung sind kein Problem):
//!
//! 1. [`check`] prüft die Wohlgeformtheit (passende Tags, Attribut-Syntax, genau ein Wurzelelement, gültige Namen,
//!    unmaskierte `<`/`&`, erlaubte Zeichen, Kommentare, Position von XML-Deklaration und DOCTYPE) und merkt sich die
//!    Elemente, deren Inhalt unverändert bleibt (keine Schema-/DTD-Prüfung):
//!    - Inhalt mit Text: gemischter Inhalt wie XHTML `<p>a <b>b</b> c</p>`, reiner Text, Referenzen, CDATA;
//!    - `xml:space="preserve"` (gilt für den ganzen Teilbaum);
//!    - Leerraum ohne Kinder (`<xsl:text> </xsl:text>`, `<textarea>\n</textarea>`) – nur Leerraum *zwischen* Kindern
//!      gilt als unbedeutend;
//!    - Geschwister, die nur durch Leerraum ohne Zeilenumbruch getrennt sind (`<b>a</b> <i>b</i>`);
//!    - Geschwister ganz ohne Leerraum dazwischen (`</b><i>`), aber nur, wenn das Dokument irgendwo gemischten Inhalt
//!      hat (dokumentartig wie XHTML). Reines Daten-XML (etwa minifiziert) wird dagegen normal formatiert.
//! 2. [`emit`] schreibt die Ausgabe.
//!
//! **Formatieren:** Elemente, Kommentare und Processing Instructions stehen je in einer Zeile, eingerückt nach
//! [`Options::indent`]; Leerraum zwischen ihnen wird durch die Einrückung ersetzt (eine Leerzeile zwischen Geschwistern
//! bleibt als eine Leerzeile erhalten). Kommentare, CDATA, PIs, XML-Deklaration und DOCTYPE (samt internem Subset)
//! bleiben Zeichen für Zeichen erhalten. Tags bleiben, wie sie geschrieben sind (Attribute, Reihenfolge,
//! Anführungszeichen, Leerraum); bei einem mehrzeiligen Start-Tag folgen die Folgezeilen der neuen Einrückung der
//! ersten Zeile (siehe [`Writer::start_tag`]). `<a/>` bleibt `<a/>`, `<a></a>` bleibt `<a></a>`. Ab 100 Ebenen
//! wird nicht weiter eingerückt ([`MAX_INDENT_DEPTH`]), damit die Ausgabe bei tiefer Verschachtelung nicht quadratisch wächst.
//! **Minifizieren:** wie Formatieren, aber ohne Zeilenumbrüche und Einrückung (Tags ganz unverändert); Kommentare
//! bleiben (sie können in XML Bedeutung haben). **Prüfen:** nur Durchlauf 1. `SortKeys` gibt es für XML nicht (Text unverändert).
//!
//! BOM bleibt erhalten, `\r\n` und einzelne `\r` werden wie vom XML-Parser vorgeschrieben zu `\n`.
//! Fehlerpositionen sind 1-basiert, Spalten in Zeichen.

use super::{FormatError, Indent, Op, Options, Outcome};
use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};

/// Siehe [`super::run`].
pub fn run(op: Op, text: &str, opts: Options, cancel: &AtomicBool, progress: &dyn Fn(f32)) -> Outcome {
    run_with(op, text, opts, false, cancel, progress)
}

/// Wie [`run`]; `fragment` = true für eine Auswahl: dann sind mehrere (oder keine) Elemente auf oberster Ebene erlaubt,
/// undefinierte Entitäten werden nicht bemängelt (die DTD steht außerhalb der Auswahl), und Text auf oberster Ebene
/// lässt die ganze Auswahl unverändert (wie gemischter Inhalt).
pub fn run_with(op: Op, text: &str, opts: Options, fragment: bool, cancel: &AtomicBool, progress: &dyn Fn(f32)) -> Outcome {
    if op == Op::SortKeys {
        return Outcome::Text(text.to_string());
    }
    let (bom, body) = match text.strip_prefix('\u{feff}') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let src: Cow<str> = if body.contains('\r') { Cow::Owned(body.replace("\r\n", "\n").replace('\r', "\n")) } else { Cow::Borrowed(body) };
    let src = src.as_ref();
    let validate = op == Op::Validate;
    let mut tick = Ticker::new(src.len(), cancel, progress, 0.0, if validate { 1.0 } else { 0.5 });
    let info = match check(src, fragment, &mut tick) {
        Ok(info) => info,
        Err(Stop::Cancelled) => return Outcome::Cancelled,
        Err(Stop::Invalid(off, message)) => return Outcome::Invalid(position(src, off, bom, message)),
    };
    if validate {
        progress(1.0);
        return Outcome::Text(text.to_string());
    }
    let mut tick = Ticker::new(src.len(), cancel, progress, 0.5, 0.5);
    match emit(src, &info, op == Op::Minify, opts, bom, &mut tick) {
        Ok(out) => {
            progress(1.0);
            Outcome::Text(out)
        }
        Err(Stop::Cancelled) => Outcome::Cancelled,
        Err(Stop::Invalid(off, message)) => Outcome::Invalid(position(src, off, bom, message)),
    }
}

enum Stop {
    Cancelled,
    /// Byte-Position im (normalisierten) Text und englische Meldung.
    Invalid(usize, String),
}

type Res<T> = Result<T, Stop>;

fn bad<T>(off: usize, msg: impl Into<String>) -> Res<T> {
    Err(Stop::Invalid(off, msg.into()))
}

/// Byte-Position → 1-basierte Zeile/Spalte (Spalte in Zeichen; ein BOM zählt als Zeichen der ersten Zeile).
fn position(src: &str, off: usize, bom: bool, message: String) -> FormatError {
    let mut off = off.min(src.len());
    while !src.is_char_boundary(off) {
        off -= 1;
    }
    let before = &src[..off];
    let line = before.bytes().filter(|&c| c == b'\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let mut column = before[line_start..].chars().count() + 1;
    if bom && line == 1 {
        column += 1;
    }
    FormatError { line, column, message }
}

/// Abbruch prüfen (alle 64 KiB) und Fortschritt melden (höchstens je 1 %).
struct Ticker<'a> {
    cancel: &'a AtomicBool,
    progress: &'a dyn Fn(f32),
    len: usize,
    base: f32,
    span: f32,
    next: usize,
    last: i32,
}

const CHECK_EVERY: usize = 64 * 1024;

impl<'a> Ticker<'a> {
    fn new(len: usize, cancel: &'a AtomicBool, progress: &'a dyn Fn(f32), base: f32, span: f32) -> Self {
        Ticker { cancel, progress, len, base, span, next: 0, last: -1 }
    }

    #[inline]
    fn at(&mut self, pos: usize) -> Res<()> {
        if pos >= self.next {
            self.slow(pos)
        } else {
            Ok(())
        }
    }

    #[cold]
    fn slow(&mut self, pos: usize) -> Res<()> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(Stop::Cancelled);
        }
        let f = (self.base + self.span * (pos as f64 / self.len.max(1) as f64) as f32).min(1.0);
        let pct = (f * 100.0) as i32;
        if pct > self.last {
            self.last = pct;
            (self.progress)(f);
        }
        self.next = pos.saturating_add(CHECK_EVERY);
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------------------------------------------

/// Attribut: Name und Wert samt Anführungszeichen (Byte-Bereiche).
struct Attr {
    name: (usize, usize),
    value: (usize, usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mk {
    Comment,
    Pi,
    Decl,
    CData,
    Doctype,
}

#[derive(Clone, Copy, Debug)]
enum Tok {
    Eof,
    /// Zeichendaten bis zum nächsten `<`; `ws` = nur Leerraum (Referenzen zählen als Text).
    Text { start: usize, end: usize, ws: bool },
    /// Start-Tag (Attribute in [`Lexer::attrs`]); `empty` = `<a/>`.
    Start { start: usize, end: usize, name: (usize, usize), empty: bool, preserve: Option<bool> },
    End { start: usize, name: (usize, usize) },
    /// Kommentar, PI, XML-Deklaration, CDATA, DOCTYPE – werden unverändert übernommen.
    Markup { start: usize, end: usize, kind: Mk },
}

struct Lexer<'a> {
    s: &'a str,
    b: &'a [u8],
    pos: usize,
    attrs: Vec<Attr>,
    seen: HashSet<&'a str>,
    /// Ohne DOCTYPE sind nur die fünf vordefinierten Entitäten erlaubt.
    check_entities: bool,
}

#[inline]
fn is_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r')
}

fn is_xml_char(v: u32) -> bool {
    matches!(v, 0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF)
}

fn is_name_start(c: char) -> bool {
    matches!(c,
        ':' | 'A'..='Z' | '_' | 'a'..='z' | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}'
        | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}'
        | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}' | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}'
        | '\u{10000}'..='\u{EFFFF}')
}

fn is_name_char(c: char) -> bool {
    is_name_start(c) || matches!(c, '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}')
}

/// Namen und Ähnliches für Meldungen kürzen.
fn short(s: &str) -> String {
    let mut it = s.chars();
    let head: String = it.by_ref().take(40).collect();
    if it.next().is_some() {
        head + "…"
    } else {
        head
    }
}

impl<'a> Lexer<'a> {
    fn new(s: &'a str, check_entities: bool) -> Self {
        Lexer { s, b: s.as_bytes(), pos: 0, attrs: Vec::new(), seen: HashSet::new(), check_entities }
    }

    fn name(&self, r: (usize, usize)) -> &'a str {
        &self.s[r.0..r.1]
    }

    fn skip_ws(&self, mut p: usize) -> usize {
        while p < self.b.len() && is_ws(self.b[p]) {
            p += 1;
        }
        p
    }

    /// Zeichen an `p` für Meldungen.
    fn char_at(&self, p: usize) -> String {
        match self.s.get(p..).and_then(|r| r.chars().next()) {
            Some(c) => format!("'{}'", c.escape_debug()),
            None => "end of input".to_string(),
        }
    }

    /// Ende eines XML-Namens ab `start` (`start`, wenn dort keiner beginnt).
    fn name_end(&self, start: usize) -> usize {
        let b = self.b;
        let mut p = start;
        while p < b.len() {
            let c = b[p];
            if c < 0x80 {
                let ok = c.is_ascii_alphabetic() || c == b'_' || c == b':' || (p > start && (c.is_ascii_digit() || c == b'-' || c == b'.'));
                if !ok {
                    break;
                }
                p += 1;
            } else {
                let ch = self.s.get(p..).and_then(|r| r.chars().next()).unwrap_or('\0');
                let ok = if p == start { is_name_start(ch) } else { is_name_char(ch) };
                if !ok {
                    break;
                }
                p += ch.len_utf8();
            }
        }
        p
    }

    fn next(&mut self) -> Res<Tok> {
        let b = self.b;
        let p = self.pos;
        if p >= b.len() {
            return Ok(Tok::Eof);
        }
        if b[p] != b'<' {
            return self.text();
        }
        let rest = &b[p..];
        if rest.starts_with(b"<?") {
            self.pi()
        } else if rest.starts_with(b"<!--") {
            self.comment()
        } else if rest.starts_with(b"<![CDATA[") {
            self.cdata()
        } else if rest.starts_with(b"<!DOCTYPE") {
            self.doctype()
        } else if rest.starts_with(b"<!") {
            bad(p, "invalid markup: expected '<!--', '<![CDATA[' or '<!DOCTYPE'")
        } else if rest.starts_with(b"</") {
            self.end_tag()
        } else {
            self.start_tag()
        }
    }

    fn text(&mut self) -> Res<Tok> {
        let b = self.b;
        let start = self.pos;
        let mut i = start;
        let mut ws = true;
        while i < b.len() {
            match b[i] {
                b'<' => break,
                b'&' => {
                    i = self.reference(i)?;
                    ws = false;
                    continue;
                }
                b' ' | b'\t' | b'\n' | b'\r' => {}
                b']' if b[i..].starts_with(b"]]>") => return bad(i, "']]>' is not allowed in text (escape '>' as '&gt;')"),
                _ => ws = false,
            }
            i += 1;
        }
        self.pos = i;
        Ok(Tok::Text { start, end: i, ws })
    }

    /// Entitäts- oder Zeichenreferenz ab `&` an `i` prüfen; liefert die Position hinter `;`.
    fn reference(&self, i: usize) -> Res<usize> {
        let b = self.b;
        let s = self.s;
        if b.get(i + 1) == Some(&b'#') {
            let hex = b.get(i + 2) == Some(&b'x');
            let radix = if hex { 16 } else { 10 };
            let digits = if hex { i + 3 } else { i + 2 };
            let mut j = digits;
            let mut v: u32 = 0;
            while let Some(d) = b.get(j).and_then(|&c| (c as char).to_digit(radix)) {
                v = v.saturating_mul(radix).saturating_add(d);
                j += 1;
            }
            if j == digits || b.get(j) != Some(&b';') {
                return bad(i, "invalid character reference (expected e.g. '&#123;' or '&#x7B;')");
            }
            if !is_xml_char(v) {
                return bad(i, format!("character reference '{}' is not a legal XML character", short(&s[i..=j])));
            }
            return Ok(j + 1);
        }
        let e = self.name_end(i + 1);
        if e == i + 1 {
            return bad(i, "'&' must be escaped as '&amp;'");
        }
        if b.get(e) != Some(&b';') {
            return bad(i, format!("entity reference '&{}' must end with ';' (escape a literal '&' as '&amp;')", short(&s[i + 1..e])));
        }
        let name = &s[i + 1..e];
        if self.check_entities && !matches!(name, "lt" | "gt" | "amp" | "apos" | "quot") {
            return bad(
                i,
                format!("undefined entity '&{};' (without a DOCTYPE only &lt; &gt; &amp; &apos; &quot; are defined)", short(name)),
            );
        }
        Ok(e + 1)
    }

    fn start_tag(&mut self) -> Res<Tok> {
        let s: &'a str = self.s;
        let b = self.b;
        let start = self.pos;
        let ns = start + 1;
        let ne = self.name_end(ns);
        if ne == ns {
            return bad(start, "'<' must start a tag (escape a literal '<' as '&lt;')");
        }
        let name = (ns, ne);
        let tag = short(&s[ns..ne]);
        self.attrs.clear();
        self.seen.clear();
        let mut preserve = None;
        let mut p = ne;
        loop {
            let before = p;
            p = self.skip_ws(p);
            let had_ws = p > before;
            match b.get(p) {
                None => return bad(start, format!("start tag <{tag}> is not closed with '>'")),
                Some(b'>') => {
                    self.pos = p + 1;
                    return Ok(Tok::Start { start, end: p + 1, name, empty: false, preserve });
                }
                Some(b'/') => {
                    if b.get(p + 1) != Some(&b'>') {
                        return bad(p, format!("expected '>' after '/' in tag <{tag}>"));
                    }
                    self.pos = p + 2;
                    return Ok(Tok::Start { start, end: p + 2, name, empty: true, preserve });
                }
                Some(_) => {}
            }
            if !had_ws {
                return bad(p, format!("expected whitespace, '>' or '/>' in tag <{tag}>, found {}", self.char_at(p)));
            }
            let an = p;
            let ae = self.name_end(an);
            if ae == an {
                return bad(p, format!("expected an attribute name in tag <{tag}>, found {}", self.char_at(p)));
            }
            let aname = &s[an..ae];
            p = self.skip_ws(ae);
            if b.get(p) != Some(&b'=') {
                return bad(p, format!("expected '=' after attribute '{}'", short(aname)));
            }
            p = self.skip_ws(p + 1);
            let q = match b.get(p) {
                Some(&q @ (b'"' | b'\'')) => q,
                _ => return bad(p, format!("value of attribute '{}' must be in quotes", short(aname))),
            };
            let vs = p;
            p += 1;
            loop {
                match b.get(p) {
                    None => return bad(vs, format!("value of attribute '{}' is not closed", short(aname))),
                    Some(&c) if c == q => break,
                    Some(b'<') => return bad(p, "'<' is not allowed in attribute values (escape it as '&lt;')"),
                    Some(b'&') => p = self.reference(p)?,
                    Some(_) => p += 1,
                }
            }
            p += 1;
            let dup = if self.attrs.len() < 16 {
                self.attrs.iter().any(|a| &s[a.name.0..a.name.1] == aname)
            } else {
                if self.seen.is_empty() {
                    for a in &self.attrs {
                        self.seen.insert(&s[a.name.0..a.name.1]);
                    }
                }
                !self.seen.insert(aname)
            };
            if dup {
                return bad(an, format!("duplicate attribute '{}'", short(aname)));
            }
            if aname == "xml:space" {
                preserve = Some(&s[vs + 1..p - 1] == "preserve");
            }
            self.attrs.push(Attr { name: (an, ae), value: (vs, p) });
        }
    }

    fn end_tag(&mut self) -> Res<Tok> {
        let start = self.pos;
        let ns = start + 2;
        let ne = self.name_end(ns);
        if ne == ns {
            return bad(ns, format!("expected an element name after '</', found {}", self.char_at(ns)));
        }
        let p = self.skip_ws(ne);
        if self.b.get(p) != Some(&b'>') {
            return bad(p, format!("expected '>' to close end tag </{}>, found {}", short(&self.s[ns..ne]), self.char_at(p)));
        }
        self.pos = p + 1;
        Ok(Tok::End { start, name: (ns, ne) })
    }

    fn comment(&mut self) -> Res<Tok> {
        let start = self.pos;
        let body = start + 4;
        let Some(rel) = self.s[body..].find("--") else {
            return bad(start, "comment is not closed (missing '-->')");
        };
        let k = body + rel;
        if self.b.get(k + 2) != Some(&b'>') {
            return bad(k, "'--' is not allowed inside a comment");
        }
        self.pos = k + 3;
        Ok(Tok::Markup { start, end: k + 3, kind: Mk::Comment })
    }

    fn cdata(&mut self) -> Res<Tok> {
        let start = self.pos;
        let body = start + 9;
        let Some(rel) = self.s[body..].find("]]>") else {
            return bad(start, "CDATA section is not closed (missing ']]>')");
        };
        self.pos = body + rel + 3;
        Ok(Tok::Markup { start, end: self.pos, kind: Mk::CData })
    }

    fn pi(&mut self) -> Res<Tok> {
        let s = self.s;
        let start = self.pos;
        let ts = start + 2;
        let te = self.name_end(ts);
        if te == ts {
            return bad(ts, format!("expected a processing instruction target after '<?', found {}", self.char_at(ts)));
        }
        let Some(rel) = s[te..].find("?>") else {
            return bad(start, "processing instruction is not closed (missing '?>')");
        };
        let end = te + rel + 2;
        let target = &s[ts..te];
        if rel > 0 && !is_ws(self.b[te]) {
            return bad(te, format!("expected whitespace after the processing instruction target '{}'", short(target)));
        }
        let kind = if target == "xml" {
            if !s[te..te + rel].trim_start_matches([' ', '\t', '\n', '\r']).starts_with("version") {
                return bad(start, "the XML declaration must start with 'version' (e.g. <?xml version=\"1.0\"?>)");
            }
            Mk::Decl
        } else if target.eq_ignore_ascii_case("xml") {
            return bad(ts, format!("the processing instruction target '{target}' is reserved"));
        } else {
            Mk::Pi
        };
        self.pos = end;
        Ok(Tok::Markup { start, end, kind })
    }

    /// DOCTYPE samt internem Subset überspringen (Anführungszeichen, Kommentare und PIs im Subset beachtet).
    fn doctype(&mut self) -> Res<Tok> {
        let b = self.b;
        let start = self.pos;
        let mut p = start + 9;
        if !b.get(p).is_some_and(|&c| is_ws(c)) {
            return bad(p, "expected whitespace after '<!DOCTYPE'");
        }
        p = self.skip_ws(p);
        let ne = self.name_end(p);
        if ne == p {
            return bad(p, format!("expected the root element name after '<!DOCTYPE', found {}", self.char_at(p)));
        }
        p = ne;
        let mut subset = false;
        while p < b.len() {
            match b[p] {
                q @ (b'"' | b'\'') => match b[p + 1..].iter().position(|&c| c == q) {
                    Some(r) => p += r + 2,
                    None => return bad(p, "quoted string in the DOCTYPE declaration is not closed"),
                },
                b'[' if !subset => {
                    subset = true;
                    p += 1;
                }
                b']' if subset => {
                    subset = false;
                    p += 1;
                }
                b'<' if subset && b[p..].starts_with(b"<!--") => match self.s[p + 4..].find("-->") {
                    Some(r) => p += 4 + r + 3,
                    None => return bad(p, "comment is not closed (missing '-->')"),
                },
                b'<' if subset && b[p..].starts_with(b"<?") => match self.s[p + 2..].find("?>") {
                    Some(r) => p += 2 + r + 2,
                    None => return bad(p, "processing instruction is not closed (missing '?>')"),
                },
                b'>' if !subset => {
                    self.pos = p + 1;
                    return Ok(Tok::Markup { start, end: p + 1, kind: Mk::Doctype });
                }
                _ => p += 1,
            }
        }
        bad(start, "DOCTYPE declaration is not closed (missing '>')")
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Durchlauf 1: Wohlgeformtheit
// ---------------------------------------------------------------------------------------------------------------

/// Ergebnis der Prüfung.
struct Info {
    /// Elemente, deren Inhalt unverändert bleibt: (Start des Start-Tags, Ende des End-Tags), nach Start sortiert.
    /// Verschachtelte Einträge in solchen Elementen werden beim Schreiben übersprungen.
    preserved: Vec<(usize, usize)>,
    /// Nur bei `fragment`: Text auf oberster Ebene → alles bleibt unverändert.
    top_text: bool,
}

/// Offenes Element in Durchlauf 1 (bzw. die oberste Ebene einer Auswahl).
struct Open {
    name: (usize, usize),
    start: usize,
    /// Text (nicht nur Leerraum), Referenzen oder CDATA direkt im Inhalt.
    has_text: bool,
    /// `xml:space="preserve"`.
    preserve: bool,
    /// Kindelemente.
    elems: bool,
    /// Kindelemente, Kommentare oder PIs.
    children: bool,
    /// Leerraum direkt im Inhalt.
    ws: bool,
    /// Direkt davor endete ein Kind (Elemente, Kommentare, PIs); `gap` = Leerraum seitdem (`Some(true)` mit Zeilenumbruch).
    prev_child: bool,
    gap: Option<bool>,
    /// Zwei Geschwister nur durch Leerraum ohne Zeilenumbruch getrennt (`<b>a</b> <i>b</i>`): bedeutsam.
    inline: bool,
    /// Zwei Geschwister ohne jeden Leerraum dazwischen (`</b><i>`).
    tight: bool,
}

impl Open {
    fn new(name: (usize, usize), start: usize, preserve: bool) -> Self {
        Open { name, start, has_text: false, preserve, elems: false, children: false, ws: false, prev_child: false, gap: None, inline: false, tight: false }
    }

    /// Ein Kind beginnt.
    fn child(&mut self) {
        if self.prev_child {
            match self.gap {
                None => self.tight = true,
                Some(false) => self.inline = true,
                Some(true) => {}
            }
        }
        self.children = true;
        self.prev_child = false;
        self.gap = None;
    }

    /// Ein Kind ist zu Ende.
    fn child_done(&mut self) {
        self.prev_child = true;
        self.gap = None;
    }

    fn ws_text(&mut self, newline: bool) {
        self.ws = true;
        if self.prev_child {
            self.gap = Some(self.gap.unwrap_or(false) || newline);
        }
    }

    fn text(&mut self) {
        self.has_text = true;
        self.prev_child = false;
        self.gap = None;
    }
}

fn parent<'s>(stack: &'s mut [Open], top: &'s mut Open) -> &'s mut Open {
    match stack.last_mut() {
        Some(o) => o,
        None => top,
    }
}

/// Erstes verbotenes Zeichen (Steuerzeichen außer Tab/LF/CR, U+FFFE, U+FFFF).
fn illegal_char(src: &str) -> Option<(usize, String)> {
    let b = src.as_bytes();
    for (i, &c) in b.iter().enumerate() {
        if c < 0x20 {
            if !matches!(c, b'\t' | b'\n' | b'\r') {
                return Some((i, format!("illegal character U+{c:04X} (control characters are not allowed in XML)")));
            }
        } else if c == 0xEF && b.get(i + 1) == Some(&0xBF) && matches!(b.get(i + 2), Some(0xBE | 0xBF)) {
            return Some((i, format!("illegal character U+FFF{} (not allowed in XML)", if b[i + 2] == 0xBE { 'E' } else { 'F' })));
        }
    }
    None
}

fn check(src: &str, fragment: bool, tick: &mut Ticker) -> Res<Info> {
    let illegal = illegal_char(src);
    match structure(src, fragment, tick) {
        Ok(info) => match illegal {
            Some((off, msg)) => bad(off, msg),
            None => Ok(info),
        },
        Err(Stop::Invalid(off, msg)) => match illegal {
            Some((o2, m2)) if o2 < off => bad(o2, m2),
            _ => bad(off, msg),
        },
        Err(e) => Err(e),
    }
}

fn structure(src: &str, fragment: bool, tick: &mut Ticker) -> Res<Info> {
    let mut lx = Lexer::new(src, !fragment);
    let mut stack: Vec<Open> = Vec::new();
    let mut preserved = Vec::new();
    // Elemente mit Geschwistern ohne Leerraum dazwischen: bleiben unverändert, wenn das Dokument gemischten Inhalt hat
    // (dokumentartig wie XHTML, wo `</b><i>` ≠ `</b>\n<i>`); in Daten-XML ohne gemischten Inhalt werden sie formatiert.
    let mut tight = Vec::new();
    let mut doc_mixed = false;
    // Oberste Ebene (zählt nur bei `fragment`).
    let mut top = Open::new((0, 0), 0, false);
    let (mut roots, mut doctype, mut top_text) = (0usize, false, false);
    loop {
        tick.at(lx.pos)?;
        match lx.next()? {
            Tok::Eof => break,
            Tok::Text { start, end, ws } => {
                if ws {
                    let nl = src.as_bytes()[start..end].contains(&b'\n');
                    parent(&mut stack, &mut top).ws_text(nl);
                    continue;
                }
                match stack.last_mut() {
                    Some(o) => o.text(),
                    None if fragment => top_text = true,
                    None => {
                        let t = &src[start..end];
                        let at = start + t.len() - t.trim_start_matches([' ', '\t', '\n', '\r']).len();
                        let msg = if roots == 0 { "text is not allowed before the root element" } else { "text is not allowed after the root element" };
                        return bad(at, msg);
                    }
                }
            }
            Tok::Markup { start, kind, .. } => match kind {
                Mk::Decl if start != 0 => return bad(start, "the XML declaration is only allowed at the very start of the document"),
                Mk::CData => match stack.last_mut() {
                    Some(o) => o.text(),
                    None if fragment => top_text = true,
                    None => return bad(start, "CDATA sections are only allowed inside the root element"),
                },
                Mk::Doctype => {
                    if doctype {
                        return bad(start, "only one DOCTYPE declaration is allowed");
                    }
                    if roots > 0 {
                        return bad(start, "the DOCTYPE declaration must come before the root element");
                    }
                    doctype = true;
                    // Entitäten können im (internen oder externen) Subset deklariert sein – externe werden nie geladen.
                    lx.check_entities = false;
                }
                Mk::Comment | Mk::Pi => {
                    let p = parent(&mut stack, &mut top);
                    p.child();
                    p.child_done();
                }
                _ => {}
            },
            Tok::Start { start, name, empty, preserve, .. } => {
                if stack.is_empty() {
                    if roots > 0 && !fragment {
                        return bad(start, format!("only one root element is allowed, found a second one: <{}>", short(lx.name(name))));
                    }
                    roots += 1;
                }
                let p = parent(&mut stack, &mut top);
                p.child();
                p.elems = true;
                if empty {
                    p.child_done();
                } else {
                    stack.push(Open::new(name, start, preserve == Some(true)));
                }
            }
            Tok::End { start, name } => {
                let Some(o) = stack.pop() else {
                    return bad(start, format!("end tag </{}> has no matching start tag", short(lx.name(name))));
                };
                if lx.name(o.name) != lx.name(name) {
                    return bad(start, format!("mismatched end tag: expected </{}>, found </{}>", short(lx.name(o.name)), short(lx.name(name))));
                }
                if (o.has_text && o.elems) || o.inline {
                    doc_mixed = true;
                }
                // Text, gemischter Inhalt, xml:space="preserve", bedeutsamer Leerraum zwischen Geschwistern sowie
                // Leerraum ohne Kinder (`<xsl:text> </xsl:text>`, `<textarea>\n</textarea>`) bleiben, wie sie sind.
                if o.has_text || o.preserve || o.inline || (o.ws && !o.children) {
                    preserved.push((o.start, lx.pos));
                } else if o.tight {
                    tight.push((o.start, lx.pos));
                }
                parent(&mut stack, &mut top).child_done();
            }
        }
    }
    if doc_mixed {
        preserved.extend(tight);
    }
    if fragment && (top.inline || (top.tight && doc_mixed)) {
        top_text = true;
    }
    if let Some(o) = stack.last() {
        return bad(o.start, format!("element <{}> is not closed", short(lx.name(o.name))));
    }
    if roots == 0 && !fragment {
        return bad(src.len(), "no root element found");
    }
    preserved.sort_unstable();
    Ok(Info { preserved, top_text })
}

// ---------------------------------------------------------------------------------------------------------------
// Durchlauf 2: Ausgabe
// ---------------------------------------------------------------------------------------------------------------

/// Tiefer als 100 Ebenen wird nicht weiter eingerückt (sonst wächst die Ausgabe quadratisch mit der Tiefe:
/// 10 000 Ebenen ergäben rund 200 MB Leerzeichen).
const MAX_INDENT_DEPTH: usize = 100;

struct Writer {
    out: String,
    unit: String,
    minify: bool,
    /// Schon etwas geschrieben (davor kein Zeilenumbruch).
    any: bool,
    /// Im Leerraum vor dem nächsten Eintrag stand eine Leerzeile.
    blank: bool,
    /// Der letzte Eintrag war ein Start-Tag mit Inhalt (direkt danach keine Leerzeile).
    after_open: bool,
}

impl Writer {
    /// Neue Zeile mit Einrückung beginnen (beim Minifizieren nichts).
    fn line(&mut self, depth: usize, closing: bool) {
        if !self.minify {
            if self.any {
                self.out.push('\n');
                if self.blank && !self.after_open && !closing {
                    self.out.push('\n');
                }
            }
            for _ in 0..depth.min(MAX_INDENT_DEPTH) {
                self.out.push_str(&self.unit);
            }
        }
        self.any = true;
        self.blank = false;
        self.after_open = false;
    }

    /// Start-Tag wie geschrieben. Steht es über mehrere Zeilen, rücken die Folgezeilen um dieselbe Differenz wie die
    /// erste Zeile (die alte Einrückung wird durch die neue ersetzt); Zeilen innerhalb von Attributwerten bleiben
    /// unverändert. Ist das nicht eindeutig (Tag stand nicht am Zeilenanfang, Folgezeile weniger oder anders
    /// eingerückt), bleibt das ganze Tag unverändert.
    fn start_tag(&mut self, src: &str, start: usize, end: usize, attrs: &[Attr], depth: usize) {
        let tag = &src[start..end];
        if self.minify || !tag.contains('\n') {
            self.out.push_str(tag);
            return;
        }
        let line_start = src[..start].rfind('\n').map_or(0, |i| i + 1);
        let old = &src[line_start..start];
        let new = self.unit.repeat(depth.min(MAX_INDENT_DEPTH));
        match shift_lines(src, start, end, attrs, old, &new) {
            Some(shifted) => self.out.push_str(&shifted),
            None => self.out.push_str(tag),
        }
    }
}

/// Folgezeilen von `src[start..end]` (außerhalb der Attributwerte) von der Einrückung `old` auf `new` umstellen;
/// `None`, wenn das nicht eindeutig geht.
fn shift_lines(src: &str, start: usize, end: usize, attrs: &[Attr], old: &str, new: &str) -> Option<String> {
    if !old.bytes().all(is_ws) {
        return None;
    }
    let mut out = String::with_capacity(end - start + 64);
    let mut last = start;
    let mut a = 0;
    for (k, _) in src[start..end].match_indices('\n') {
        let nl = start + k;
        while a < attrs.len() && attrs[a].value.1 <= nl {
            a += 1;
        }
        if a < attrs.len() && attrs[a].value.0 < nl {
            continue; // Zeilenumbruch im Attributwert
        }
        let ls = nl + 1;
        let line = src[ls..end].split('\n').next().unwrap_or("");
        if line.bytes().all(is_ws) {
            continue; // leere Zeile bleibt
        }
        if !line.starts_with(old) {
            return None;
        }
        out.push_str(&src[last..ls]);
        out.push_str(new);
        last = ls + old.len();
    }
    out.push_str(&src[last..end]);
    Some(out)
}

fn emit(src: &str, info: &Info, minify: bool, opts: Options, bom: bool, tick: &mut Ticker) -> Res<String> {
    let unit = match opts.indent {
        Indent::Spaces(n) => " ".repeat(n as usize),
        Indent::Tab => "\t".to_string(),
    };
    let mut w = Writer { out: String::with_capacity(src.len() + src.len() / 4 + 16), unit, minify, any: false, blank: false, after_open: false };
    if bom {
        w.out.push('\u{feff}');
    }
    if info.top_text {
        w.out.push_str(src);
        return Ok(w.out);
    }
    let bytes = src.as_bytes();
    let pre = &info.preserved;
    let mut cur = 0;
    let mut depth = 0usize;
    let mut lx = Lexer::new(src, false);
    loop {
        tick.at(lx.pos)?;
        match lx.next()? {
            Tok::Eof => break,
            Tok::Text { start, end, ws } => {
                if ws {
                    if bytes[start..end].iter().filter(|&&c| c == b'\n').count() >= 2 {
                        w.blank = true;
                    }
                } else {
                    // Kommt nach der Prüfung nicht vor (Text steht nur in unverändert übernommenen Elementen).
                    w.line(depth, false);
                    w.out.push_str(&src[start..end]);
                }
            }
            Tok::Markup { start, end, .. } => {
                w.line(depth, false);
                w.out.push_str(&src[start..end]);
            }
            Tok::End { start, .. } => {
                depth = depth.saturating_sub(1);
                w.line(depth, true);
                w.out.push_str(&src[start..lx.pos]);
            }
            Tok::Start { start, end, empty, .. } => {
                w.line(depth, false);
                w.start_tag(src, start, end, &lx.attrs, depth);
                while cur < pre.len() && pre[cur].0 < start {
                    cur += 1;
                }
                if cur < pre.len() && pre[cur].0 == start {
                    // Gemischter Inhalt, Text oder xml:space="preserve": Inhalt und End-Tag wie geschrieben.
                    let stop = pre[cur].1;
                    w.out.push_str(&src[end..stop]);
                    lx.pos = stop;
                    cur += 1;
                    continue;
                }
                if !empty {
                    let p = lx.skip_ws(end);
                    if bytes[p..].starts_with(b"</") {
                        // Leeres Element (höchstens Leerraum): `<a></a>` in einer Zeile.
                        lx.pos = p;
                        lx.next()?;
                        w.out.push_str(&src[p..lx.pos]);
                    } else {
                        depth += 1;
                        w.after_open = true;
                    }
                }
            }
        }
    }
    if opts.final_newline && w.any {
        w.out.push('\n');
    }
    Ok(w.out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Instant;

    fn go(op: Op, s: &str, opts: Options) -> Outcome {
        run(op, s, opts, &AtomicBool::new(false), &|_| {})
    }

    fn text(o: Outcome) -> String {
        match o {
            Outcome::Text(t) => t,
            o => panic!("expected text, got {o:?}"),
        }
    }

    fn fmt(s: &str) -> String {
        text(go(Op::Format, s, Options::default()))
    }

    fn fmt_with(s: &str, indent: Indent, final_newline: bool) -> String {
        text(go(Op::Format, s, Options { indent, final_newline }))
    }

    fn mini(s: &str) -> String {
        text(go(Op::Minify, s, Options { indent: Indent::Spaces(2), final_newline: false }))
    }

    fn err(s: &str) -> FormatError {
        match go(Op::Validate, s, Options::default()) {
            Outcome::Invalid(e) => {
                // Formatieren und Minifizieren melden denselben Fehler.
                assert_eq!(go(Op::Format, s, Options::default()), Outcome::Invalid(e.clone()));
                assert_eq!(go(Op::Minify, s, Options::default()), Outcome::Invalid(e.clone()));
                e
            }
            o => panic!("expected invalid for {s:?}, got {o:?}"),
        }
    }

    fn at(s: &str) -> (usize, usize) {
        let e = err(s);
        (e.line, e.column)
    }

    #[test]
    fn format_basic_indentation() {
        let input = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<root><a x=\"1\"  y='2'><b>text</b><c/><d></d><e>  \n </e></a></root>";
        let want = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<root>\n  <a x=\"1\"  y='2'>\n    <b>text</b>\n    <c/>\n    <d></d>\n    <e>  \n </e>\n  </a>\n</root>\n";
        assert_eq!(fmt(input), want);
        assert_eq!(fmt_with(input, Indent::Spaces(4), true), want.replace("    ", "        ").replace("\n  <", "\n    <"));
        assert_eq!(fmt_with("<r><a/></r>", Indent::Tab, false), "<r>\n\t<a/>\n</r>");
        // Schon formatiert: unverändert.
        assert_eq!(fmt(want), want);
    }

    #[test]
    fn format_keeps_comments_pis_and_declaration() {
        let input = "<?xml version=\"1.0\"?>\n<?xml-stylesheet type=\"text/xsl\" href=\"s.xsl\"?>\n<!-- top -->\n<r><!--  inner  --><?app  do  this ?><a/>\n<!-- multi\n     line -->\n</r>\n<!-- after -->";
        let want = "<?xml version=\"1.0\"?>\n<?xml-stylesheet type=\"text/xsl\" href=\"s.xsl\"?>\n<!-- top -->\n<r>\n  <!--  inner  -->\n  <?app  do  this ?>\n  <a/>\n  <!-- multi\n     line -->\n</r>\n<!-- after -->\n";
        assert_eq!(fmt(input), want);
    }

    #[test]
    fn format_keeps_doctype_with_internal_subset() {
        let doctype = "<!DOCTYPE note [\n  <!ENTITY writer \"Jane > Doe\">\n  <!-- a comment with > and ' -->\n  <?pi in subset > ?>\n  <!ELEMENT note (#PCDATA|to|from)*>\n  <!ATTLIST note lang CDATA 'en'>\n]>";
        let input = format!("{doctype}\n<note><to>&writer;</to>\n   <from>Me</from></note>");
        let want = format!("{doctype}\n<note>\n  <to>&writer;</to>\n  <from>Me</from>\n</note>\n");
        assert_eq!(fmt(&input), want);
        let external = "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Strict//EN\" \"http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd\">";
        assert_eq!(fmt(&format!("{external}<html><p>&nbsp;</p></html>")), format!("{external}\n<html>\n  <p>&nbsp;</p>\n</html>\n"));
    }

    #[test]
    fn format_keeps_cdata() {
        let input = "<r><s><![CDATA[ if (a < b && c) { x = \"]\"; } ]]></s>\n<t>\n  <![CDATA[x]]>\n  <u/>\n</t></r>";
        let want = "<r>\n  <s><![CDATA[ if (a < b && c) { x = \"]\"; } ]]></s>\n  <t>\n  <![CDATA[x]]>\n  <u/>\n</t>\n</r>\n";
        assert_eq!(fmt(input), want);
    }

    #[test]
    fn format_keeps_mixed_content() {
        let input = "<html xmlns=\"http://www.w3.org/1999/xhtml\"><body><p>Hello <b>bold</b>  and <i>it\n </i>.</p>\n<div>\n<p>x</p></div>\n<p><b>a</b> <i>b</i></p>\n<p>\n<b>a</b>\n<i>b</i></p></body></html>";
        let want = "<html xmlns=\"http://www.w3.org/1999/xhtml\">\n  <body>\n    <p>Hello <b>bold</b>  and <i>it\n </i>.</p>\n    <div>\n      <p>x</p>\n    </div>\n    <p><b>a</b> <i>b</i></p>\n    <p>\n      <b>a</b>\n      <i>b</i>\n    </p>\n  </body>\n</html>\n";
        // Leerzeichen ohne Zeilenumbruch zwischen Geschwistern ist bedeutsam; mit Zeilenumbruch wird formatiert.
        assert_eq!(fmt(input), want);
        // Text vor dem ersten Kind, Text erst nach dem letzten Kind.
        assert_eq!(fmt("<r>\n<p>a<b/></p>\n<q><b/>tail</q></r>"), "<r>\n  <p>a<b/></p>\n  <q><b/>tail</q>\n</r>\n");
        // Gemischter Inhalt im Dokument: Geschwister ohne Leerraum (`</p><q>`) bleiben zusammen.
        assert_eq!(fmt("<r><p>a<b/></p><q><b/>tail</q></r>"), "<r><p>a<b/></p><q><b/>tail</q></r>\n");
        // Referenzen zählen als Text.
        assert_eq!(fmt("<r><p>&#32;<b/></p></r>"), "<r>\n  <p>&#32;<b/></p>\n</r>\n");
    }

    #[test]
    fn format_keeps_xml_space_preserve() {
        let input = "<r><pre xml:space=\"preserve\">\n  <a>  <b/> </a>\n    <c xml:space=\"default\">  <d/> </c>\n</pre><x>  <y/></x><z xml:space='preserve'>  </z></r>";
        let want = "<r>\n  <pre xml:space=\"preserve\">\n  <a>  <b/> </a>\n    <c xml:space=\"default\">  <d/> </c>\n</pre>\n  <x>\n    <y/>\n  </x>\n  <z xml:space='preserve'>  </z>\n</r>\n";
        assert_eq!(fmt(input), want);
        assert_eq!(fmt("<r><a xml:space=\"default\">  <b/></a></r>"), "<r>\n  <a xml:space=\"default\">\n    <b/>\n  </a>\n</r>\n");
    }

    #[test]
    fn format_namespaces_and_attributes() {
        let input = "<x:r xmlns:x=\"urn:x\" xmlns=\"urn:d\"><x:a x:attr=\"1\"/><b\n   one=\"1\"\n   two='2 &amp; &#x41;'\n   three = \"a\nb\"\n   /></x:r>";
        let want = "<x:r xmlns:x=\"urn:x\" xmlns=\"urn:d\">\n  <x:a x:attr=\"1\"/>\n  <b\n   one=\"1\"\n   two='2 &amp; &#x41;'\n   three = \"a\nb\"\n   />\n</x:r>\n";
        assert_eq!(fmt(input), want);
        assert_eq!(fmt("<a  >  </a  >"), "<a  >  </a  >\n");
        assert_eq!(fmt("<a  ></a  >"), "<a  ></a  >\n");
        assert_eq!(fmt("<ä ö=\"ü\"><ß/></ä>"), "<ä ö=\"ü\">\n  <ß/>\n</ä>\n");
    }

    #[test]
    fn multi_line_start_tags_follow_the_new_indentation() {
        // Folgezeilen rücken um dieselbe Differenz wie die erste Zeile; Zeilen in Attributwerten bleiben.
        let input = "<r>\n<a\n  one=\"1\"\n  two=\"x\n y\"\n/>\n    <b\n      c = '1'\n\n        d=\"2\">\n    </b>\n</r>";
        let want = "<r>\n  <a\n    one=\"1\"\n    two=\"x\n y\"\n  />\n  <b\n    c = '1'\n\n      d=\"2\">\n    </b>\n</r>\n";
        assert_eq!(fmt(input), want);
        assert_eq!(fmt(want), want);
        assert_eq!(fmt_with("<r>\n<a\n  x=\"1\"/></r>", Indent::Tab, false), "<r>\n\t<a\n\t  x=\"1\"/>\n</r>");
        // Mehrdeutig (Folgezeile weniger eingerückt als die erste, Tag nicht am Zeilenanfang): unverändert.
        assert_eq!(fmt("<r>\n    <a\n  x=\"1\"/>\n</r>"), "<r>\n  <a\n  x=\"1\"/>\n</r>\n");
        assert_eq!(fmt("<r><a\n   x=\"1\"/></r>"), "<r>\n  <a\n   x=\"1\"/>\n</r>\n");
        assert_eq!(fmt("<r>\n  <a\n   x=\"1\"/>\n</r>"), "<r>\n  <a\n   x=\"1\"/>\n</r>\n");
        // Auch das Start-Tag eines Elements mit gemischtem Inhalt rückt mit, der Inhalt nicht.
        assert_eq!(fmt("<r>\n<p\n  class=\"x\">a\n <b>b</b></p></r>"), "<r>\n  <p\n    class=\"x\">a\n <b>b</b></p>\n</r>\n");
        // Minifizieren lässt Tags ganz unverändert.
        assert_eq!(mini("<r>\n  <a\n    x=\"1\"/>\n</r>"), "<r><a\n    x=\"1\"/></r>");
    }

    #[test]
    fn whitespace_only_content_is_kept() {
        // XSLT: Leerraum in xsl:text ist Ausgabe.
        let x = "<xsl:stylesheet xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\" version=\"1.0\"><xsl:template match=\"/\"><xsl:text>\n</xsl:text><xsl:text> </xsl:text></xsl:template></xsl:stylesheet>";
        let want = "<xsl:stylesheet xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\" version=\"1.0\">\n  <xsl:template match=\"/\">\n    <xsl:text>\n</xsl:text>\n    <xsl:text> </xsl:text>\n  </xsl:template>\n</xsl:stylesheet>\n";
        assert_eq!(fmt(x), want);
        assert_eq!(fmt(want), want);
        assert_eq!(mini(x), x);
        assert_eq!(mini("<form><textarea> </textarea>\n<tspan>\n</tspan></form>"), "<form><textarea> </textarea><tspan>\n</tspan></form>");
        // Nur Leerraum zwischen Kindern (auch Kommentaren) ist unbedeutend.
        assert_eq!(fmt("<a> <!-- c --> </a>"), "<a>\n  <!-- c -->\n</a>\n");
        assert_eq!(mini("<a> <!-- c -->\n <b/> </a>"), "<a><!-- c --><b/></a>");
        // Leerzeichen ohne Zeilenumbruch zwischen Kommentar und Element zählt wie zwischen Elementen.
        assert_eq!(mini("<a> <!-- c --> <b/> </a>"), "<a> <!-- c --> <b/> </a>");
    }

    #[test]
    fn inline_siblings_are_kept_together() {
        // Leerzeichen ohne Zeilenumbruch zwischen Geschwistern: Element bleibt, wie es ist.
        assert_eq!(fmt("<r>\n<p><b>a</b> <i>b</i></p>\n</r>"), "<r>\n  <p><b>a</b> <i>b</i></p>\n</r>\n");
        assert_eq!(mini("<r>\n<p><b>a</b> <i>b</i></p>\n</r>"), "<r><p><b>a</b> <i>b</i></p></r>");
        // Dokument mit gemischtem Inhalt: kein Zeilenumbruch zwischen `</b><i>`.
        let doc = "<div>\n<p>x <b>y</b></p>\n<p><b>a</b><i>b</i></p>\n</div>";
        assert_eq!(fmt(doc), "<div>\n  <p>x <b>y</b></p>\n  <p><b>a</b><i>b</i></p>\n</div>\n");
        // Daten-XML ohne gemischten Inhalt (z. B. minifiziert): wird normal formatiert.
        assert_eq!(fmt("<r><a>1</a><b><c/><d/></b></r>"), "<r>\n  <a>1</a>\n  <b>\n    <c/>\n    <d/>\n  </b>\n</r>\n");
        // Auswahl: Text bzw. Inline-Geschwister auf oberster Ebene → unverändert.
        let opts = Options { indent: Indent::Spaces(2), final_newline: false };
        let c = AtomicBool::new(false);
        assert_eq!(text(run_with(Op::Format, "<b>a</b> <i>b</i>", opts, true, &c, &|_| {})), "<b>a</b> <i>b</i>");
        assert_eq!(text(run_with(Op::Minify, "<b>a</b> <i>b</i>", opts, true, &c, &|_| {})), "<b>a</b> <i>b</i>");
        assert_eq!(text(run_with(Op::Format, "<a/>\n<b/>", opts, true, &c, &|_| {})), "<a/>\n<b/>");
    }

    #[test]
    fn format_entities_kept_as_written() {
        let input = "<r><a>&lt;&gt;&amp;&apos;&quot;&#169;&#xA9;</a><b c=\"&#x26;&lt;\"/></r>";
        assert_eq!(fmt(input), "<r>\n  <a>&lt;&gt;&amp;&apos;&quot;&#169;&#xA9;</a>\n  <b c=\"&#x26;&lt;\"/>\n</r>\n");
    }

    #[test]
    fn format_blank_lines_kept_once() {
        let input = "<r>\n\n\n  <a/>\n\n\n\n  <b/>\n  <c/>\n\n</r>\n\n<!-- x -->";
        assert_eq!(fmt(input), "<r>\n  <a/>\n\n  <b/>\n  <c/>\n</r>\n\n<!-- x -->\n");
    }

    #[test]
    fn format_bom_crlf_and_final_newline() {
        let input = "\u{feff}<?xml version=\"1.0\"?>\r\n<r>\r\n<a>x\r\ny</a>\r\n</r>\r\n";
        assert_eq!(fmt(input), "\u{feff}<?xml version=\"1.0\"?>\n<r>\n  <a>x\ny</a>\n</r>\n");
        assert_eq!(fmt_with(input, Indent::Spaces(2), false), "\u{feff}<?xml version=\"1.0\"?>\n<r>\n  <a>x\ny</a>\n</r>");
        assert_eq!(fmt("<r>a\rb</r>"), "<r>a\nb</r>\n");
        // Prüfen gibt den Text unverändert zurück (auch mit BOM und CRLF).
        assert_eq!(text(go(Op::Validate, input, Options::default())), input);
        // BOM zählt als Zeichen in Zeile 1.
        assert_eq!(at("\u{feff}<a></b>"), (1, 5));
    }

    #[test]
    fn sort_keys_returns_text_unchanged() {
        let input = "<r>  <b/><a/></r>";
        assert_eq!(text(go(Op::SortKeys, input, Options::default())), input);
        assert!(!super::super::supports(super::super::Kind::Xml, Op::SortKeys));
    }

    #[test]
    fn minify_removes_whitespace_between_elements() {
        let input = "<?xml version=\"1.0\"?>\n<!DOCTYPE r [\n <!ENTITY e \"v\">\n]>\n<r>\n  <!-- keep -->\n  <a  x=\"1\" >\n    <b/>\n  </a>\n  <p>Text <i>mixed</i> here</p>\n  <pre xml:space=\"preserve\"> <q/> </pre>\n  <e>   </e>\n  <t> text </t>\n</r>\n";
        assert_eq!(
            mini(input),
            "<?xml version=\"1.0\"?><!DOCTYPE r [\n <!ENTITY e \"v\">\n]><r><!-- keep --><a  x=\"1\" ><b/></a><p>Text <i>mixed</i> here</p><pre xml:space=\"preserve\"> <q/> </pre><e>   </e><t> text </t></r>"
        );
        assert_eq!(text(go(Op::Minify, "<r> <a/> </r>", Options::default())), "<r><a/></r>\n");
        // Gemischter Inhalt bleibt beim Minifizieren unverändert (`<b>a</b> <i>b</i>` ≠ `<b>a</b><i>b</i>`).
        assert_eq!(mini("<p><b>a</b> <i>b</i></p>"), "<p><b>a</b> <i>b</i></p>");
        // Daten-XML: minifizieren und wieder formatieren ergibt das Formatierte.
        let data = "<r>\n  <!-- c -->\n  <a x=\"1\">\n    <b/>\n    <c>1</c>\n  </a>\n  <d></d>\n</r>\n";
        assert_eq!(mini(data), "<r><!-- c --><a x=\"1\"><b/><c>1</c></a><d></d></r>");
        assert_eq!(fmt(&mini(data)), data);
    }

    #[test]
    fn validate_ok_returns_input() {
        for s in ["<a/>", "<a>x</a>\n", "<?xml version='1.0'?><!-- c --><a b=\"&amp;\"><![CDATA[<>&]]></a><?pi?>", "<!DOCTYPE a><a>&custom;</a>"] {
            assert_eq!(text(go(Op::Validate, s, Options::default())), s, "{s}");
        }
    }

    #[test]
    fn validate_errors_with_positions() {
        assert_eq!(at("<a><b></a>"), (1, 7));
        assert!(err("<a><b></a>").message.contains("expected </b>, found </a>"));
        assert_eq!(at("<a>\n  <b>\n</a>"), (3, 1));
        assert_eq!(at("<a>"), (1, 1));
        assert!(err("<a><b>").message.contains("<b> is not closed"));
        assert_eq!(at("</a>"), (1, 1));
        assert_eq!(at("<a/><b/>"), (1, 5));
        assert!(err("<a/><b/>").message.contains("only one root element"));
        assert_eq!(at("<a/>text"), (1, 5));
        assert_eq!(at("  x<a/>"), (1, 3));
        assert_eq!(at(""), (1, 1));
        assert!(err("  \n ").message.contains("no root element"));
        assert_eq!(at("<a>1 < 2</a>"), (1, 6));
        assert_eq!(at("<a>AT&T</a>"), (1, 6));
        assert_eq!(at("<a>a & b</a>"), (1, 6));
        assert!(err("<a>a & b</a>").message.contains("&amp;"));
        assert_eq!(at("<a>&nbsp;</a>"), (1, 4));
        assert!(err("<a>&nbsp;</a>").message.contains("undefined entity"));
        assert_eq!(at("<a>&#0;</a>"), (1, 4));
        assert_eq!(at("<a>&#xZ;</a>"), (1, 4));
        assert_eq!(at("<a x=1/>"), (1, 6));
        assert!(err("<a x=1/>").message.contains("quotes"));
        assert_eq!(at("<a x=\"1\" x=\"2\"/>"), (1, 10));
        assert!(err("<a x=\"1\" x=\"2\"/>").message.contains("duplicate attribute 'x'"));
        assert_eq!(at("<a x=\"1\"y=\"2\"/>"), (1, 9));
        assert_eq!(at("<a x/>"), (1, 5));
        assert_eq!(at("<a x=\"<\"/>"), (1, 7));
        assert_eq!(at("<a x=\"1/>"), (1, 6));
        assert_eq!(at("<1a/>"), (1, 1));
        assert_eq!(at("<a!/>"), (1, 3));
        assert_eq!(at("<a></a b>"), (1, 8));
        assert_eq!(at("<a><!-- x -- y --></a>"), (1, 11));
        assert_eq!(at("<a><!-- x </a>"), (1, 4));
        assert_eq!(at("<a>]]></a>"), (1, 4));
        assert_eq!(at("<a><![CDATA[x</a>"), (1, 4));
        assert_eq!(at("<a><!ELEMENT></a>"), (1, 4));
        assert_eq!(at("\n<?xml version=\"1.0\"?><a/>"), (2, 1));
        assert_eq!(at("<?xml encoding=\"x\"?><a/>"), (1, 1));
        assert_eq!(at("<a><?XML x?></a>"), (1, 6));
        assert_eq!(at("<a><?pi"), (1, 4));
        assert_eq!(at("<a>\u{1}</a>"), (1, 4));
        assert_eq!(at("<a>\u{fffe}</a>"), (1, 4));
        assert_eq!(at("<a/><!DOCTYPE a>"), (1, 5));
        assert_eq!(at("<!DOCTYPE a><!DOCTYPE a><a/>"), (1, 13));
        assert_eq!(at("<!DOCTYPE a [ <!ENTITY x \"y\">"), (1, 1));
        assert_eq!(at("<![CDATA[x]]><a/>"), (1, 1));
        // Steuerzeichen nach einem früheren Strukturfehler: der frühere gewinnt.
        assert_eq!(at("<a></b>\u{1}"), (1, 4));
        // CRLF zählt als ein Zeilenumbruch.
        assert_eq!(at("<a>\r\n<b>\r\n</a>"), (3, 1));
        assert_eq!(at("<a>\r<b>\r</a>"), (3, 1));
    }

    #[test]
    fn validate_positions_count_characters() {
        // `<äöü>ß` sind 6 Zeichen (10 Bytes) → `</äöx>` beginnt in Spalte 7.
        assert_eq!(at("<äöü>ß</äöx>"), (1, 7));
        assert_eq!(at("<r>\n  <ü>😀</x></r>"), (2, 7));
        assert_eq!(at("<r>日本語 & </r>"), (1, 8));
        // Ungültiges Namenszeichen mitten im Namen (nach einem Mehrbyte-Zeichen).
        assert_eq!(at("<aé×b/>"), (1, 4));
    }

    #[test]
    fn invalid_input_is_not_changed() {
        assert!(matches!(go(Op::Format, "<a><b></a>", Options::default()), Outcome::Invalid(_)));
        assert!(matches!(go(Op::Minify, "<a>", Options::default()), Outcome::Invalid(_)));
    }

    #[test]
    fn fragment_mode_allows_several_top_level_elements() {
        let c = AtomicBool::new(false);
        let opts = Options { indent: Indent::Spaces(2), final_newline: false };
        let f = |op: Op, s: &str| text(run_with(op, s, opts, true, &c, &|_| {}));
        assert_eq!(f(Op::Format, "<a><b/></a>\n <c/>"), "<a>\n  <b/>\n</a>\n<c/>");
        assert_eq!(f(Op::Format, "a <b>x</b> c"), "a <b>x</b> c");
        assert_eq!(f(Op::Format, "<a>&nbsp;</a>"), "<a>&nbsp;</a>");
        assert_eq!(f(Op::Format, ""), "");
        assert_eq!(f(Op::Minify, "<a> <b/> </a>\n<c/>"), "<a><b/></a><c/>");
        assert!(matches!(run_with(Op::Validate, "<a><b></a>", opts, true, &c, &|_| {}), Outcome::Invalid(_)));
        // Ohne Fragment-Modus: zwei Wurzeln sind ein Fehler.
        assert!(matches!(go(Op::Validate, "<a/><c/>", opts), Outcome::Invalid(_)));
    }

    fn sample() -> String {
        concat!(
            "\u{feff}<?xml version=\"1.0\" encoding=\"utf-8\"?>\n",
            "<!DOCTYPE doc [ <!ENTITY e \"x>y\"> <!-- ] > --> ]>\n",
            "<doc xmlns:x=\"urn:x\">\n\n",
            "<x:meta  a='1'   b=\"2\"/>\n",
            "    <!-- note -->\n  <?proc data?>\n",
            "<p>Text with <b>bold</b> &e; and &#x263A;</p>\n",
            "<list><item>1</item>\n\n\n<item>2</item>\n<item/>\n   <item></item></list>\n",
            "<x:inline><b>a</b> <i>b</i></x:inline>\n",
            "<code xml:space=\"preserve\">  keep   <this/>  </code>\n",
            "<s><![CDATA[ <raw> ]]></s>\n",
            "</doc>\n"
        )
        .to_string()
    }

    #[test]
    fn format_and_minify_are_idempotent() {
        let s = sample();
        let once = fmt(&s);
        assert_eq!(fmt(&once), once);
        assert_eq!(text(go(Op::Format, &once, Options { indent: Indent::Tab, final_newline: true })), fmt_with(&s, Indent::Tab, true));
        let m = mini(&s);
        assert_eq!(mini(&m), m);
        assert_eq!(mini(&once), m);
        assert!(once.starts_with('\u{feff}'));
        assert!(once.contains("<p>Text with <b>bold</b> &e; and &#x263A;</p>"));
        assert!(once.contains("  <code xml:space=\"preserve\">  keep   <this/>  </code>\n"));
        assert!(once.contains("  <x:meta  a='1'   b=\"2\"/>\n"));
        assert!(once.contains("    <item>1</item>\n\n    <item>2</item>\n    <item/>\n    <item></item>\n"));
        assert!(once.contains("  <x:inline><b>a</b> <i>b</i></x:inline>\n"));
    }

    #[test]
    fn cancel_stops_the_run() {
        let c = AtomicBool::new(true);
        for op in [Op::Format, Op::Minify, Op::Validate] {
            assert_eq!(run(op, "<a/>", Options::default(), &c, &|_| {}), Outcome::Cancelled);
        }
        // Abbruch mitten im Lauf (aus dem Fortschritt heraus).
        let big = big_doc(2 << 20);
        let c = AtomicBool::new(false);
        let seen = Mutex::new(0.0f32);
        let out = run(Op::Format, &big, Options::default(), &c, &|f| {
            *seen.lock().unwrap() = f;
            if f >= 0.6 {
                c.store(true, Ordering::Relaxed);
            }
        });
        assert_eq!(out, Outcome::Cancelled);
        assert!(*seen.lock().unwrap() < 0.8);
    }

    #[test]
    fn progress_is_monotonic_and_coarse() {
        let big = big_doc(4 << 20);
        for op in [Op::Format, Op::Validate] {
            let vals = Mutex::new(Vec::new());
            let out = run(op, &big, Options::default(), &AtomicBool::new(false), &|f| vals.lock().unwrap().push(f));
            assert!(matches!(out, Outcome::Text(_)));
            let v = vals.into_inner().unwrap();
            assert!(v.len() > 20 && v.len() <= 103, "{} progress calls", v.len());
            assert!(v.windows(2).all(|w| w[0] <= w[1]));
            assert!(v.iter().all(|f| (0.0..=1.0).contains(f)));
            assert_eq!(*v.last().unwrap(), 1.0);
        }
    }

    /// Dokument mit etwa `size` Bytes aus allem, was vorkommt.
    fn big_doc(size: usize) -> String {
        let mut s = String::with_capacity(size + 512);
        s.push_str("<?xml version=\"1.0\"?>\n<!-- generated -->\n<root xmlns:n=\"urn:n\">\n");
        let mut i = 0u64;
        while s.len() < size {
            s.push_str(&format!(
                "  <item id=\"{i}\" n:name='näme {i}'>\n    <title>Title {i} &amp; more</title><p>Mixed <b>bold</b> text 😀</p>\n    <empty/><e2></e2>\n    <!-- comment {i} --><s><![CDATA[<x>]]></s>\n  </item>\n  <group><a>1</a><b><c><d/></c></b></group>\n"
            ));
            i += 1;
        }
        s.push_str("</root>\n");
        s
    }

    #[test]
    fn large_input_is_fast() {
        let size = if cfg!(debug_assertions) { 5 << 20 } else { 50 << 20 };
        let big = big_doc(size);
        let c = AtomicBool::new(false);
        for op in [Op::Validate, Op::Format, Op::Minify] {
            let t = Instant::now();
            let out = run(op, &big, Options::default(), &c, &|_| {});
            let dt = t.elapsed();
            eprintln!("xml {op:?} {} MB: {dt:?}", big.len() >> 20);
            let Outcome::Text(t) = out else { panic!("{op:?} failed") };
            assert!(!t.is_empty());
            // Allein unter 1 s; die Grenze ist großzügig, weil die ganze Test-Suite parallel läuft.
            if !cfg!(debug_assertions) {
                assert!(dt.as_secs_f64() < 60.0, "{op:?} took {dt:?}");
            }
        }
    }

    #[test]
    fn deep_nesting_without_stack_overflow() {
        let n = 10_000;
        let deep = format!("{}{}", "<a>".repeat(n), "</a>".repeat(n));
        assert_eq!(text(go(Op::Validate, &deep, Options::default())), deep);
        assert_eq!(mini(&deep), "<a>".repeat(n - 1) + "<a></a>" + &"</a>".repeat(n - 1));
        let flat = text(go(Op::Format, &deep, Options { indent: Indent::Spaces(0), final_newline: false }));
        assert_eq!(flat.lines().count(), 2 * n - 1);
        // Mit Einrückung: ab MAX_INDENT_DEPTH bleibt sie gleich, die Ausgabe wächst nur linear.
        let out = fmt(&deep);
        assert!(out.len() < 3 * deep.len() + 2 * n * (2 * MAX_INDENT_DEPTH + 1), "{} bytes", out.len());
        assert_eq!(out.lines().nth(50), Some(format!("{}<a>", "  ".repeat(50)).as_str()));
        assert_eq!(out.lines().nth(5_000), Some(format!("{}<a>", "  ".repeat(MAX_INDENT_DEPTH)).as_str()));
        assert_eq!(fmt(&out), out);
        let m = 80;
        let deep2 = format!("{}{}", "<a>".repeat(m), "</a>".repeat(m));
        let out = fmt(&deep2);
        assert_eq!(out.lines().count(), 2 * m - 1);
        assert_eq!(out.lines().nth(m - 1), Some(format!("{}<a></a>", "  ".repeat(m - 1)).as_str()));
        // Gemischter Inhalt ganz oben: alles wird unverändert übernommen.
        let mixed = format!("<r>x{}{}</r>", "<a>".repeat(n), "</a>".repeat(n));
        assert_eq!(fmt(&mixed), format!("{mixed}\n"));
        // Nicht geschlossen in großer Tiefe.
        assert_eq!(at(&"<a>".repeat(n)), (1, 3 * (n - 1) + 1));
    }

    /// Einfacher Pseudo-Zufall (xorshift), damit der Test reproduzierbar bleibt.
    struct Rng(u64);
    impl Rng {
        fn step(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: usize) -> usize {
            (self.step() % n as u64) as usize
        }
    }

    #[test]
    fn fuzz_never_panics_and_stays_consistent() {
        const PIECES: &[&str] = &[
            "<a>", "</a>", "<b x='1'>", "</b>", "<c/>", "<c />", "text", " ", "\n", "\n\n", "\t", "&amp;", "&", "&#x41;", "&#0;", "&lt", "<", ">", "<!--",
            "-->", "--", "<![CDATA[", "]]>", "<?pi ", "?>", "<?xml version=\"1.0\"?>", "<!DOCTYPE a [", "]>", "<!DOCTYPE a>", "\"", "'", "=", "ä",
            "😀", "\r\n", "\r", " xml:space=\"preserve\"", "<p>", "</p>", "<n:x xmlns:n='u'>", "</n:x>", "\u{1}", "/", "!", "<!", "</", "<a", "<a b=\"",
            "\u{feff}", "]", "#", ";", "x:y", "\u{fffe}",
        ];
        let c = AtomicBool::new(false);
        let opts = Options::default();
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for round in 0..20_000 {
            let mut s = String::new();
            if round % 3 == 0 {
                // Gültiges Gerüst mit zufälligem Inhalt.
                s.push_str("<r>");
            }
            for _ in 0..rng.below(30) {
                if round % 5 == 4 {
                    s.push(char::from_u32(rng.below(0x3000) as u32).unwrap_or('?'));
                } else {
                    s.push_str(PIECES[rng.below(PIECES.len())]);
                }
            }
            if round % 3 == 0 {
                s.push_str("</r>");
            }
            let v = run(Op::Validate, &s, opts, &c, &|_| {});
            let f = run(Op::Format, &s, opts, &c, &|_| {});
            let m = run(Op::Minify, &s, opts, &c, &|_| {});
            for frag in [Op::Format, Op::Minify, Op::Validate] {
                let _ = run_with(frag, &s, opts, true, &c, &|_| {});
            }
            match (&v, &f, &m) {
                (Outcome::Text(vt), Outcome::Text(ft), Outcome::Text(mt)) => {
                    assert_eq!(vt, &s);
                    assert_eq!(&fmt(ft), ft, "format not idempotent for {s:?}");
                    // Mehrzeilige Start-Tags rücken beim Formatieren mit, beim Minifizieren nicht.
                    if !s.contains(['\n', '\r']) {
                        assert_eq!(&text(go(Op::Minify, ft, opts)), mt, "minify(format(x)) != minify(x) for {s:?}");
                    }
                    assert_eq!(&text(go(Op::Minify, mt, opts)), mt, "minify not idempotent for {s:?}");
                    assert!(matches!(go(Op::Validate, mt, opts), Outcome::Text(_)));
                }
                (Outcome::Invalid(e), Outcome::Invalid(e2), Outcome::Invalid(e3)) => {
                    assert_eq!(e, e2);
                    assert_eq!(e, e3);
                    assert!(e.line >= 1 && e.column >= 1);
                }
                _ => panic!("inconsistent outcomes for {s:?}: {v:?} {f:?} {m:?}"),
            }
        }
    }
}
