//! Markdown-Teilmenge: AST und Parser (ohne externe Abhängigkeit).
//!
//! Unterstützt: Überschriften, Absätze, Codeblöcke, (verschachtelte) Listen, Aufgabenlisten, Zitate,
//! Tabellen (GitHub-Stil), horizontale Linien sowie Fett/Kursiv/Code/Links/Umbrüche und Ticket-Verweise.
//! Der Parser ist gegen beliebige Eingaben abgesichert: begrenzte Verschachtelungstiefe, begrenzter Aufwand,
//! keine Indexzugriffe ohne Prüfung – er darf nie in Panik geraten (die App läuft mit `panic = "abort"`).

use std::cell::Cell;

/// Höchste Verschachtelung von Blöcken (Zitat in Liste in Zitat …); tiefer wird als Absatz gelesen.
const MAX_BLOCK_DEPTH: usize = 12;
/// Höchste Verschachtelung von Fett/Kursiv.
const MAX_INLINE_DEPTH: u8 = 8;
/// Aufwandsgrenze pro Absatz (Zeichenschritte); danach wird nicht mehr nach Paaren gesucht.
const INLINE_BUDGET: u32 = 3_000_000;
/// Mehr Begrenzer (`*`/`_`-Läufe) pro Absatz werden nicht ausgewertet.
const MAX_DELIMS: usize = 4000;

/// Ein Dokument: Folge von Blöcken.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Doc {
    pub blocks: Vec<Block>,
}

/// Spaltenausrichtung einer Tabelle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Align {
    /// Keine Angabe (wird linksbündig gezeichnet).
    #[default]
    None,
    Left,
    Center,
    Right,
}

/// Ein Eintrag einer Liste.
#[derive(Clone, Debug, PartialEq)]
pub struct ListItem {
    /// `Some(erledigt)` bei Aufgabenlisten (`- [ ]` / `- [x]`).
    pub task: Option<bool>,
    pub blocks: Vec<Block>,
}

/// Blockelemente.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    /// Überschrift, `level` 1–6.
    Heading { level: u8, inlines: Vec<Inline> },
    Paragraph(Vec<Inline>),
    /// Codeblock mit optionaler Sprachangabe (leer, wenn keine).
    CodeBlock { lang: String, code: String },
    /// Liste; `start` ist die erste Nummer bei nummerierten Listen.
    List { ordered: bool, start: u64, items: Vec<ListItem> },
    Quote(Vec<Block>),
    Table { align: Vec<Align>, header: Vec<Vec<Inline>>, rows: Vec<Vec<Vec<Inline>>> },
    Rule,
}

/// Inline-Elemente.
#[derive(Clone, Debug, PartialEq)]
pub enum Inline {
    Text(String),
    Bold(Vec<Inline>),
    Italic(Vec<Inline>),
    Code(String),
    Link { text: Vec<Inline>, url: String },
    /// Verweis auf ein Ticket wie `PAN-12` (nur erkannt, nicht aufgelöst).
    TicketRef(String),
    /// Weicher Umbruch (Zeilenende im Quelltext).
    SoftBreak,
    /// Harter Umbruch (zwei Leerzeichen oder Backslash am Zeilenende).
    HardBreak,
}

/// Parst Markdown. `known_prefix` entscheidet, ob ein Kürzel (z. B. `PAN`) als Ticket-Verweis
/// gilt; Muster `KÜRZEL-ZAHL` mit unbekanntem Kürzel bleiben normaler Text.
pub fn parse(src: &str, known_prefix: &dyn Fn(&str) -> bool) -> Doc {
    let lines = normalize(src);
    let p = BlockParser { prefix: known_prefix };
    Doc { blocks: p.blocks(&lines, 0) }
}

/// Parst Markdown ohne Ticket-Verweise.
pub fn parse_plain(src: &str) -> Doc {
    parse(src, &|_| false)
}

/// Reiner Text eines Inline-Abschnitts (für Tests und Spaltenbreiten).
pub fn inline_text(inlines: &[Inline]) -> String {
    let mut s = String::new();
    for i in inlines {
        match i {
            Inline::Text(t) | Inline::Code(t) | Inline::TicketRef(t) => s.push_str(t),
            Inline::Bold(c) | Inline::Italic(c) => s.push_str(&inline_text(c)),
            Inline::Link { text, .. } => s.push_str(&inline_text(text)),
            Inline::SoftBreak => s.push(' '),
            Inline::HardBreak => s.push('\n'),
        }
    }
    s
}

/// Nur diese Adressarten werden von der Oberfläche angeklickt/geöffnet (kein `file:`, keine Programme).
pub fn is_safe_url(url: &str) -> bool {
    let u = url.trim();
    let lower = u.to_ascii_lowercase();
    (lower.starts_with("http://") && u.len() > 7)
        || (lower.starts_with("https://") && u.len() > 8)
        || (lower.starts_with("mailto:") && u.len() > 7)
}

// ───────────────────────────── Blöcke ─────────────────────────────

/// Zeilenenden vereinheitlichen, Tabs am Zeilenanfang in 4 Leerzeichen wandeln.
fn normalize(src: &str) -> Vec<String> {
    let s = src.replace("\r\n", "\n").replace('\r', "\n");
    s.split('\n')
        .map(|l| {
            let rest = l.trim_start_matches([' ', '\t']);
            let lead = &l[..l.len() - rest.len()];
            if lead.contains('\t') {
                let mut cols = 0usize;
                for ch in lead.chars() {
                    cols += if ch == '\t' { 4 - cols % 4 } else { 1 };
                }
                format!("{}{}", " ".repeat(cols), rest)
            } else {
                l.to_string()
            }
        })
        .collect()
}

fn indent_of(line: &str) -> usize {
    line.bytes().take_while(|b| *b == b' ').count()
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

struct Fence {
    ch: char,
    len: usize,
    info: String,
}

fn fence_open(line: &str) -> Option<Fence> {
    let ind = indent_of(line);
    if ind > 3 {
        return None;
    }
    let rest = &line[ind..];
    let ch = rest.chars().next()?;
    if ch != '`' && ch != '~' {
        return None;
    }
    let len = rest.chars().take_while(|c| *c == ch).count();
    if len < 3 {
        return None;
    }
    let info = rest[len..].trim();
    if ch == '`' && info.contains('`') {
        return None;
    }
    Some(Fence { ch, len, info: info.split_whitespace().next().unwrap_or("").to_string() })
}

fn fence_close(line: &str, f: &Fence) -> bool {
    let ind = indent_of(line);
    if ind > 3 {
        return false;
    }
    let rest = line[ind..].trim_end();
    rest.chars().count() >= f.len && rest.chars().all(|c| c == f.ch)
}

fn heading(line: &str) -> Option<(u8, String)> {
    let ind = indent_of(line);
    if ind > 3 {
        return None;
    }
    let rest = &line[ind..];
    let n = rest.chars().take_while(|c| *c == '#').count();
    if n == 0 || n > 6 {
        return None;
    }
    let after = &rest[n..];
    if !after.is_empty() && !after.starts_with(' ') && !after.starts_with('\t') {
        return None;
    }
    let mut content = after.trim().to_string();
    // schließende #-Folge entfernen („## Titel ##“)
    let stripped = content.trim_end_matches('#');
    if stripped.len() != content.len() && (stripped.is_empty() || stripped.ends_with(' ')) {
        content = stripped.trim_end().to_string();
    }
    Some((n as u8, content))
}

fn is_hr(line: &str) -> bool {
    let ind = indent_of(line);
    if ind > 3 {
        return false;
    }
    let mut ch = None;
    let mut n = 0;
    for c in line[ind..].chars() {
        match c {
            ' ' => {}
            '-' | '*' | '_' => {
                if ch.is_some_and(|k| k != c) {
                    return false;
                }
                ch = Some(c);
                n += 1;
            }
            _ => return false,
        }
    }
    n >= 3
}

fn is_quote(line: &str) -> bool {
    let ind = indent_of(line);
    ind <= 3 && line[ind..].starts_with('>')
}

struct Marker {
    indent: usize,
    ordered: bool,
    num: u64,
    /// Spalte, ab der der Inhalt des Eintrags beginnt.
    content_indent: usize,
    /// Text der ersten Zeile hinter dem Marker.
    rest: String,
}

fn list_marker(line: &str) -> Option<Marker> {
    let ind = indent_of(line);
    if ind > 3 {
        return None;
    }
    let s = &line[ind..];
    let mut chars = s.char_indices();
    let (_, c0) = chars.next()?;
    let (ordered, num, mlen) = if c0 == '-' || c0 == '*' || c0 == '+' {
        (false, 0u64, 1usize)
    } else if c0.is_ascii_digit() {
        let digits = s.bytes().take_while(|b| b.is_ascii_digit()).count();
        if digits > 9 {
            return None;
        }
        let d = s.as_bytes().get(digits).copied()?;
        if d != b'.' && d != b')' {
            return None;
        }
        (true, s[..digits].parse::<u64>().ok()?, digits + 1)
    } else {
        return None;
    };
    let after = &s[mlen..];
    if !after.is_empty() && !after.starts_with(' ') {
        return None;
    }
    let spaces = after.bytes().take_while(|b| *b == b' ').count();
    let blank_rest = after.trim().is_empty();
    let pad = if blank_rest || spaces > 4 { 1 } else { spaces.max(1) };
    let rest_start = if blank_rest { after.len() } else { spaces.min(after.len()) };
    Some(Marker { indent: ind, ordered, num, content_indent: ind + mlen + pad, rest: after[rest_start..].to_string() })
}

/// Unterbricht die Zeile einen laufenden Absatz?
fn interrupts_paragraph(line: &str) -> bool {
    if fence_open(line).is_some() || heading(line).is_some() || is_hr(line) || is_quote(line) {
        return true;
    }
    match list_marker(line) {
        Some(m) => !m.rest.trim().is_empty() && (!m.ordered || m.num == 1),
        None => false,
    }
}

/// Zellen einer Tabellenzeile.
fn split_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut chars = t.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                cur.push('|');
                chars.next();
            }
            '|' => cells.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    // Rest nur zählen, wenn die Zeile nicht auf `|` endet (dann ist er leer)
    if !cur.trim().is_empty() || !t.ends_with('|') {
        cells.push(cur);
    }
    cells.into_iter().map(|c| c.trim().to_string()).collect()
}

fn table_delim(line: &str) -> Option<Vec<Align>> {
    if !line.contains('|') {
        return None;
    }
    let cells = split_row(line);
    if cells.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for c in cells {
        let left = c.starts_with(':');
        let right = c.ends_with(':') && c.len() > 1;
        let core = c.trim_matches(':');
        if core.is_empty() || !core.chars().all(|ch| ch == '-') {
            return None;
        }
        out.push(match (left, right) {
            (true, true) => Align::Center,
            (true, false) => Align::Left,
            (false, true) => Align::Right,
            _ => Align::None,
        });
    }
    Some(out)
}

fn table_starts_at(lines: &[String], i: usize) -> Option<Vec<Align>> {
    let head = lines.get(i)?;
    if !head.contains('|') || is_blank(head) {
        return None;
    }
    let align = table_delim(lines.get(i + 1)?)?;
    if split_row(head).len() == align.len() { Some(align) } else { None }
}

struct BlockParser<'a> {
    prefix: &'a dyn Fn(&str) -> bool,
}

impl BlockParser<'_> {
    fn inlines(&self, s: &str) -> Vec<Inline> {
        let chars: Vec<char> = s.chars().collect();
        let il = Inliner { c: &chars, prefix: self.prefix, budget: Cell::new(INLINE_BUDGET) };
        il.run(false, 0)
    }

    fn blocks(&self, lines: &[String], depth: usize) -> Vec<Block> {
        if depth > MAX_BLOCK_DEPTH {
            // zu tief verschachtelt: nur noch als Text lesen
            let text = lines.iter().map(|l| l.trim()).collect::<Vec<_>>().join("\n");
            let t = text.trim();
            return if t.is_empty() { Vec::new() } else { vec![Block::Paragraph(self.inlines(t))] };
        }
        let mut out = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            let line = &lines[i];
            if is_blank(line) {
                i += 1;
                continue;
            }
            // Codeblock
            if let Some(f) = fence_open(line) {
                let ind = indent_of(line);
                let mut code = Vec::new();
                i += 1;
                while i < lines.len() && !fence_close(&lines[i], &f) {
                    let l = &lines[i];
                    let strip = indent_of(l).min(ind);
                    code.push(l[strip..].to_string());
                    i += 1;
                }
                i += 1; // schließende Zeile (oder Ende)
                out.push(Block::CodeBlock { lang: f.info, code: code.join("\n") });
                continue;
            }
            // Überschrift
            if let Some((level, content)) = heading(line) {
                out.push(Block::Heading { level, inlines: self.inlines(&content) });
                i += 1;
                continue;
            }
            // horizontale Linie (vor Listen: `* * *`, `---`)
            if is_hr(line) {
                out.push(Block::Rule);
                i += 1;
                continue;
            }
            // Zitat
            if is_quote(line) {
                let mut inner = Vec::new();
                while i < lines.len() && is_quote(&lines[i]) {
                    let l = &lines[i];
                    let rest = &l[indent_of(l) + 1..];
                    inner.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
                    i += 1;
                }
                out.push(Block::Quote(self.blocks(&inner, depth + 1)));
                continue;
            }
            // Liste
            if list_marker(line).is_some() {
                let (block, next) = self.list(lines, i, depth);
                out.push(block);
                i = next.max(i + 1);
                continue;
            }
            // Tabelle
            if let Some(align) = table_starts_at(lines, i) {
                let cols = align.len();
                let fit = |cells: Vec<String>, me: &Self| -> Vec<Vec<Inline>> {
                    (0..cols).map(|c| me.inlines(cells.get(c).map(|s| s.as_str()).unwrap_or(""))).collect()
                };
                let header = fit(split_row(line), self);
                i += 2;
                let mut rows = Vec::new();
                while i < lines.len() && !is_blank(&lines[i]) && !interrupts_paragraph(&lines[i]) {
                    rows.push(fit(split_row(&lines[i]), self));
                    i += 1;
                }
                out.push(Block::Table { align, header, rows });
                continue;
            }
            // Absatz
            let mut text: Vec<&str> = vec![lines[i].trim_start()];
            i += 1;
            while i < lines.len() && !is_blank(&lines[i]) && !interrupts_paragraph(&lines[i]) && table_starts_at(lines, i).is_none() {
                text.push(lines[i].trim_start());
                i += 1;
            }
            let joined = text.join("\n");
            out.push(Block::Paragraph(self.inlines(joined.trim_end())));
        }
        out
    }

    /// Liest eine Liste ab `lines[start]`; gibt den Block und die nächste Zeile zurück.
    fn list(&self, lines: &[String], start: usize, depth: usize) -> (Block, usize) {
        let first = match list_marker(&lines[start]) {
            Some(m) => m,
            None => return (Block::Rule, start + 1),
        };
        let ordered = first.ordered;
        let start_num = first.num;
        let mut items = Vec::new();
        let mut i = start;
        while i < lines.len() {
            let m = match list_marker(&lines[i]) {
                Some(m) if m.ordered == ordered && !is_hr(&lines[i]) => m,
                _ => break,
            };
            let mut item_lines = vec![m.rest.clone()];
            let mut j = i + 1;
            let mut blanks = 0usize;
            while j < lines.len() {
                let l = &lines[j];
                if is_blank(l) {
                    blanks += 1;
                    j += 1;
                    continue;
                }
                let ind = indent_of(l);
                let nested_marker = ind >= 2 && ind > m.indent && list_marker(l).is_some();
                let belongs = if ind >= m.content_indent || nested_marker {
                    true
                } else {
                    // „faule“ Fortsetzung: direkt anschließende Textzeile ohne Einrückung
                    blanks == 0 && !interrupts_paragraph(l) && list_marker(l).is_none() && table_starts_at(lines, j).is_none()
                };
                if !belongs {
                    break;
                }
                for _ in 0..blanks {
                    item_lines.push(String::new());
                }
                blanks = 0;
                let strip = ind.min(m.content_indent);
                item_lines.push(l[strip..].to_string());
                j += 1;
            }
            // Aufgabe?
            let mut task = None;
            if let Some(first_line) = item_lines.first_mut() {
                let t = first_line.trim_start();
                for (tag, done) in [("[ ]", false), ("[x]", true), ("[X]", true)] {
                    if let Some(r) = t.strip_prefix(tag) {
                        if r.is_empty() || r.starts_with(' ') {
                            task = Some(done);
                            *first_line = r.trim_start().to_string();
                            break;
                        }
                    }
                }
            }
            items.push(ListItem { task, blocks: self.blocks(&item_lines, depth + 1) });
            i = j.max(i + 1);
        }
        (Block::List { ordered, start: start_num, items }, i)
    }
}

// ───────────────────────────── Inline ─────────────────────────────

struct Delim {
    ch: char,
    count: usize,
    orig: usize,
    can_open: bool,
    can_close: bool,
}

enum Item {
    /// Fertiger Knoten samt Verschachtelungstiefe.
    Node(Inline, u8),
    Delim(Delim),
}

struct Inliner<'a> {
    c: &'a [char],
    prefix: &'a dyn Fn(&str) -> bool,
    budget: Cell<u32>,
}

fn is_ws(c: Option<char>) -> bool {
    c.is_none_or(|c| c.is_whitespace())
}

fn is_punct(c: Option<char>) -> bool {
    c.is_some_and(|c| !c.is_alphanumeric() && !c.is_whitespace())
}

impl Inliner<'_> {
    fn tick(&self, n: usize) -> bool {
        let b = self.budget.get();
        let n = n.min(u32::MAX as usize) as u32;
        if b < n {
            self.budget.set(0);
            false
        } else {
            self.budget.set(b - n);
            true
        }
    }

    fn at(&self, i: usize) -> Option<char> {
        self.c.get(i).copied()
    }

    /// Länge der Folge gleicher Zeichen ab `i`.
    fn run_len(&self, i: usize) -> usize {
        let Some(ch) = self.at(i) else { return 0 };
        self.c[i..].iter().take_while(|c| **c == ch).count()
    }

    /// Hinter dem Code-Span, der bei `i` beginnt (falls er schließt): (Inhalt, Index danach).
    fn code_span(&self, i: usize) -> Option<(String, usize)> {
        let n = self.run_len(i);
        if n == 0 || self.at(i) != Some('`') {
            return None;
        }
        let mut j = i + n;
        while j < self.c.len() {
            if !self.tick(1) {
                return None;
            }
            if self.c[j] == '`' {
                let m = self.run_len(j);
                if m == n {
                    let raw: String = self.c[i + n..j].iter().map(|c| if *c == '\n' { ' ' } else { *c }).collect();
                    let s = if raw.len() >= 2 && raw.starts_with(' ') && raw.ends_with(' ') && !raw.trim().is_empty() {
                        raw[1..raw.len() - 1].to_string()
                    } else {
                        raw
                    };
                    return Some((s, j + m));
                }
                j += m;
            } else {
                j += 1;
            }
        }
        None
    }

    /// Index der passenden `]` zur `[` bei `i` (verschachtelt, Code/Escapes übersprungen).
    fn bracket_end(&self, i: usize) -> Option<usize> {
        let mut depth = 0usize;
        let mut j = i;
        while j < self.c.len() {
            if !self.tick(1) {
                return None;
            }
            match self.c[j] {
                '\\' => j += 1,
                '`' => {
                    if let Some((_, e)) = self.code_span(j) {
                        j = e;
                        continue;
                    }
                    j += self.run_len(j);
                    continue;
                }
                '[' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(j);
                    }
                }
                _ => {}
            }
            j += 1;
        }
        None
    }

    /// `(url "titel")` ab `j` (zeigt auf `(`): (Adresse, Index hinter `)`).
    fn link_dest(&self, j: usize) -> Option<(String, usize)> {
        let mut k = j + 1;
        while self.at(k).is_some_and(|c| c == ' ' || c == '\n') {
            k += 1;
        }
        let mut url = String::new();
        if self.at(k) == Some('<') {
            k += 1;
            while let Some(c) = self.at(k) {
                if !self.tick(1) || c == '\n' {
                    return None;
                }
                if c == '>' {
                    break;
                }
                url.push(c);
                k += 1;
            }
            if self.at(k) != Some('>') {
                return None;
            }
            k += 1;
        } else {
            let mut depth = 0usize;
            while let Some(c) = self.at(k) {
                if !self.tick(1) {
                    return None;
                }
                if c.is_whitespace() || c.is_control() {
                    break;
                }
                if c == '\\' && self.at(k + 1).is_some_and(|n| n.is_ascii_punctuation()) {
                    url.push(self.c[k + 1]);
                    k += 2;
                    continue;
                }
                if c == '(' {
                    depth += 1;
                } else if c == ')' {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                url.push(c);
                k += 1;
            }
        }
        while self.at(k).is_some_and(|c| c == ' ' || c == '\n') {
            k += 1;
        }
        // optionaler Titel
        if let Some(q) = self.at(k).filter(|c| *c == '"' || *c == '\'') {
            k += 1;
            while let Some(c) = self.at(k) {
                if !self.tick(1) {
                    return None;
                }
                if c == '\\' {
                    k += 2;
                    continue;
                }
                if c == q {
                    break;
                }
                k += 1;
            }
            if self.at(k) != Some(q) {
                return None;
            }
            k += 1;
            while self.at(k).is_some_and(|c| c == ' ' || c == '\n') {
                k += 1;
            }
        }
        if self.at(k) == Some(')') { Some((url, k + 1)) } else { None }
    }

    /// Bare Adresse (`http://…`/`https://…`) ab `i`: (Adresse, Index danach).
    fn bare_url(&self, i: usize) -> Option<(String, usize)> {
        let rest = &self.c[i..];
        let starts = |p: &str| rest.len() > p.len() && rest.iter().zip(p.chars()).take(p.len()).all(|(a, b)| a.to_ascii_lowercase() == b);
        if !(starts("http://") || starts("https://")) {
            return None;
        }
        if i > 0 && self.c[i - 1].is_alphanumeric() {
            return None;
        }
        let mut e = i;
        while e < self.c.len() && !self.c[e].is_whitespace() && self.c[e] != '<' && self.c[e] != '>' {
            e += 1;
        }
        // Satzzeichen am Ende gehören nicht zur Adresse
        while e > i {
            let last = self.c[e - 1];
            let strip = match last {
                '.' | ',' | ':' | ';' | '!' | '?' | '\'' | '"' | '*' | '_' | '~' => true,
                ')' => {
                    let opens = self.c[i..e].iter().filter(|c| **c == '(').count();
                    let closes = self.c[i..e].iter().filter(|c| **c == ')').count();
                    closes > opens
                }
                ']' => {
                    let opens = self.c[i..e].iter().filter(|c| **c == '[').count();
                    let closes = self.c[i..e].iter().filter(|c| **c == ']').count();
                    closes > opens
                }
                _ => false,
            };
            if strip {
                e -= 1;
            } else {
                break;
            }
        }
        let url: String = self.c[i..e].iter().collect();
        let after_scheme = url.find("://").map(|p| p + 3).unwrap_or(0);
        if url.len() <= after_scheme {
            return None;
        }
        Some((url, e))
    }

    /// Ticket-Verweis `ABC-123` ab `i` (Großbuchstaben 2–10, Bindestrich, 1–9 Ziffern).
    fn ticket(&self, i: usize) -> Option<(String, usize)> {
        let first = self.at(i)?;
        if !first.is_ascii_uppercase() {
            return None;
        }
        // Wortgrenze davor
        if let Some(p) = i.checked_sub(1).and_then(|k| self.at(k)) {
            let ok = p.is_whitespace() || matches!(p, '(' | '[' | '{' | '"' | '\'' | '*' | '_' | '~' | ',' | ';' | ':' | '>' | '|' | '“' | '„' | '‚');
            if !ok {
                return None;
            }
        }
        let letters = self.c[i..].iter().take_while(|c| c.is_ascii_uppercase()).count();
        if !(2..=10).contains(&letters) || self.at(i + letters) != Some('-') {
            return None;
        }
        let ds = i + letters + 1;
        let digits = self.c[ds..].iter().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 || digits > 9 {
            return None;
        }
        let end = ds + digits;
        // Wortgrenze dahinter
        match self.at(end) {
            None => {}
            Some(n) if n.is_whitespace() => {}
            Some(n) if matches!(n, ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}' | '"' | '\'' | '*' | '_' | '~' | '>' | '|' | '”' | '“') => {}
            Some('.') => {
                // „PAN-12.“ am Satzende ja, „PAN-12.5“ / „PAN-12.txt“ nein
                match self.at(end + 1) {
                    None => {}
                    Some(n) if n.is_whitespace() || n.is_ascii_punctuation() && !n.is_alphanumeric() => {}
                    _ => return None,
                }
            }
            _ => return None,
        }
        let name: String = self.c[i..i + letters].iter().collect();
        if !(self.prefix)(&name) {
            return None;
        }
        Some((self.c[i..end].iter().collect(), end))
    }

    /// Zerlegt den Text und wertet Betonungen aus.
    fn run(&self, in_link: bool, depth: usize) -> Vec<Inline> {
        let mut items: Vec<Item> = Vec::new();
        let mut buf = String::new();
        let mut delims = 0usize;
        let flush = |buf: &mut String, items: &mut Vec<Item>| {
            if !buf.is_empty() {
                items.push(Item::Node(Inline::Text(std::mem::take(buf)), 0));
            }
        };
        let mut i = 0;
        let n = self.c.len();
        while i < n {
            let ch = self.c[i];
            match ch {
                '\\' => match self.at(i + 1) {
                    Some('\n') => {
                        flush(&mut buf, &mut items);
                        items.push(Item::Node(Inline::HardBreak, 0));
                        i += 2;
                    }
                    Some(nx) if nx.is_ascii_punctuation() => {
                        buf.push(nx);
                        i += 2;
                    }
                    _ => {
                        buf.push('\\');
                        i += 1;
                    }
                },
                '`' => match self.code_span(i) {
                    Some((code, e)) => {
                        flush(&mut buf, &mut items);
                        items.push(Item::Node(Inline::Code(code), 0));
                        i = e;
                    }
                    None => {
                        let m = self.run_len(i);
                        buf.extend(std::iter::repeat_n('`', m));
                        i += m;
                    }
                },
                '*' | '_' => {
                    let m = self.run_len(i);
                    let prev = if i == 0 { None } else { self.at(i - 1) };
                    let next = self.at(i + m);
                    let left = !is_ws(next) && (!is_punct(next) || is_ws(prev) || is_punct(prev));
                    let right = !is_ws(prev) && (!is_punct(prev) || is_ws(next) || is_punct(next));
                    let (can_open, can_close) = if ch == '*' {
                        (left, right)
                    } else {
                        (left && (!right || is_punct(prev)), right && (!left || is_punct(next)))
                    };
                    if (can_open || can_close) && delims < MAX_DELIMS {
                        flush(&mut buf, &mut items);
                        items.push(Item::Delim(Delim { ch, count: m, orig: m, can_open, can_close }));
                        delims += 1;
                    } else {
                        buf.extend(std::iter::repeat_n(ch, m));
                    }
                    i += m;
                }
                '[' if !in_link => {
                    let link = self.bracket_end(i).and_then(|close| {
                        if self.at(close + 1) == Some('(') { self.link_dest(close + 1).map(|(u, e)| (close, u, e)) } else { None }
                    });
                    match link {
                        Some((close, url, e)) => {
                            flush(&mut buf, &mut items);
                            let sub = Inliner { c: &self.c[i + 1..close], prefix: self.prefix, budget: Cell::new(self.budget.get()) };
                            let text = if depth >= 2 { vec![Inline::Text(sub.c.iter().collect())] } else { sub.run(true, depth + 1) };
                            self.budget.set(sub.budget.get());
                            items.push(Item::Node(Inline::Link { text, url }, 1));
                            i = e;
                        }
                        None => {
                            buf.push('[');
                            i += 1;
                        }
                    }
                }
                '<' if !in_link => {
                    // <https://…>
                    let mut e = i + 1;
                    while e < n && self.c[e] != '>' && !self.c[e].is_whitespace() && self.c[e] != '<' {
                        e += 1;
                    }
                    let inner: String = self.c[i + 1..e.min(n)].iter().collect();
                    let lower = inner.to_ascii_lowercase();
                    if self.at(e) == Some('>') && (lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:")) && inner.len() > 7 {
                        flush(&mut buf, &mut items);
                        items.push(Item::Node(Inline::Link { text: vec![Inline::Text(inner.clone())], url: inner }, 1));
                        i = e + 1;
                    } else {
                        buf.push('<');
                        i += 1;
                    }
                }
                '\n' => {
                    let trailing = buf.chars().rev().take_while(|c| *c == ' ').count();
                    let hard = trailing >= 2;
                    let keep = buf.trim_end_matches(' ').len();
                    buf.truncate(keep);
                    flush(&mut buf, &mut items);
                    items.push(Item::Node(if hard { Inline::HardBreak } else { Inline::SoftBreak }, 0));
                    i += 1;
                    while self.at(i) == Some(' ') {
                        i += 1;
                    }
                }
                'h' | 'H' if !in_link && self.bare_url(i).is_some() => {
                    if let Some((url, e)) = self.bare_url(i) {
                        flush(&mut buf, &mut items);
                        items.push(Item::Node(Inline::Link { text: vec![Inline::Text(url.clone())], url }, 1));
                        i = e;
                    }
                }
                c if c.is_ascii_uppercase() && !in_link => match self.ticket(i) {
                    Some((t, e)) => {
                        flush(&mut buf, &mut items);
                        items.push(Item::Node(Inline::TicketRef(t), 1));
                        i = e;
                    }
                    None => {
                        buf.push(c);
                        i += 1;
                    }
                },
                c => {
                    buf.push(c);
                    i += 1;
                }
            }
        }
        flush(&mut buf, &mut items);
        if self.budget.get() > 0 {
            process_emphasis(&mut items);
        }
        merge_text(flatten(items).0)
    }
}

/// Alle Reste (Begrenzer ohne Gegenstück) werden zu Text.
fn flatten(items: Vec<Item>) -> (Vec<Inline>, u8) {
    let mut out = Vec::with_capacity(items.len());
    let mut depth = 0;
    for it in items {
        match it {
            Item::Node(n, d) => {
                depth = depth.max(d);
                out.push(n);
            }
            Item::Delim(d) => out.push(Inline::Text(std::iter::repeat_n(d.ch, d.count).collect())),
        }
    }
    (out, depth)
}

fn merge_text(v: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::with_capacity(v.len());
    for i in v {
        if let (Inline::Text(t), Some(Inline::Text(prev))) = (&i, out.last_mut()) {
            prev.push_str(t);
            continue;
        }
        out.push(i);
    }
    out
}

/// Betonungen nach dem Begrenzer-Verfahren von CommonMark (vereinfacht).
fn process_emphasis(items: &mut Vec<Item>) {
    let mut ci = 0;
    while ci < items.len() {
        let (ch, ccount, corig, copen) = match &items[ci] {
            Item::Delim(d) if d.can_close => (d.ch, d.count, d.orig, d.can_open),
            _ => {
                ci += 1;
                continue;
            }
        };
        let mut found = None;
        let mut oi = ci;
        while oi > 0 {
            oi -= 1;
            if let Item::Delim(o) = &items[oi] {
                if o.ch == ch && o.can_open {
                    let odd = (o.can_close || copen) && (o.orig + corig) % 3 == 0 && !(o.orig % 3 == 0 && corig % 3 == 0);
                    if !odd {
                        found = Some(oi);
                        break;
                    }
                }
            }
        }
        let Some(oi) = found else {
            ci += 1;
            continue;
        };
        let ocount = match &items[oi] {
            Item::Delim(o) => o.count,
            _ => 0,
        };
        let use_n = if ocount >= 2 && ccount >= 2 { 2 } else { 1 };
        let inner: Vec<Item> = items.drain(oi + 1..ci).collect();
        let (children, depth) = flatten(inner);
        let children = merge_text(children);
        let marker: String = std::iter::repeat_n(ch, use_n).collect();
        let mut ins: Vec<Item> = Vec::new();
        if depth + 1 > MAX_INLINE_DEPTH {
            // zu tief: Zeichen bleiben als Text stehen
            ins.push(Item::Node(Inline::Text(marker.clone()), 0));
            ins.extend(children.into_iter().map(|c| Item::Node(c, 0)));
            ins.push(Item::Node(Inline::Text(marker), 0));
        } else {
            let node = if use_n == 2 { Inline::Bold(children) } else { Inline::Italic(children) };
            ins.push(Item::Node(node, depth + 1));
        }
        let ins_len = ins.len();
        items.splice(oi + 1..oi + 1, ins);
        ci = oi + 1 + ins_len;
        let mut closer_empty = false;
        if let Some(Item::Delim(c)) = items.get_mut(ci) {
            c.count -= use_n;
            closer_empty = c.count == 0;
        }
        if closer_empty {
            items.remove(ci);
        }
        let mut opener_empty = false;
        if let Some(Item::Delim(o)) = items.get_mut(oi) {
            o.count -= use_n;
            opener_empty = o.count == 0;
        }
        if opener_empty {
            items.remove(oi);
            ci -= 1;
        }
    }
}

// ───────────────────────────── Tests ─────────────────────────────

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn p(s: &str) -> Doc {
        parse_plain(s)
    }
    fn pt(s: &str) -> Doc {
        parse(s, &|x| x == "PAN" || x == "DEV")
    }
    fn t(s: &str) -> Inline {
        Inline::Text(s.to_string())
    }
    fn para(d: &Doc) -> &Vec<Inline> {
        match d.blocks.first() {
            Some(Block::Paragraph(v)) => v,
            other => panic!("kein Absatz: {other:?}"),
        }
    }

    #[test]
    fn empty_and_blank() {
        assert!(p("").blocks.is_empty());
        assert!(p("\n\n   \n").blocks.is_empty());
    }

    #[test]
    fn headings() {
        let d = p("# A\n## B ##\n### C\n#### D\n#NoHeading\n####### too many");
        assert_eq!(d.blocks.len(), 5); // die letzten beiden Zeilen bilden einen Absatz
        assert_eq!(d.blocks[0], Block::Heading { level: 1, inlines: vec![t("A")] });
        assert_eq!(d.blocks[1], Block::Heading { level: 2, inlines: vec![t("B")] });
        assert_eq!(d.blocks[2], Block::Heading { level: 3, inlines: vec![t("C")] });
        assert_eq!(d.blocks[3], Block::Heading { level: 4, inlines: vec![t("D")] });
        assert!(matches!(d.blocks[4], Block::Paragraph(_)));
        // leere Überschrift
        assert_eq!(p("#").blocks[0], Block::Heading { level: 1, inlines: vec![] });
    }

    #[test]
    fn emphasis_basic() {
        let d = p("a **b** *c* ***d*** `e` f");
        assert_eq!(
            para(&d),
            &vec![
                t("a "),
                Inline::Bold(vec![t("b")]),
                t(" "),
                Inline::Italic(vec![t("c")]),
                t(" "),
                Inline::Italic(vec![Inline::Bold(vec![t("d")])]),
                t(" "),
                Inline::Code("e".into()),
                t(" f"),
            ]
        );
        assert_eq!(para(&p("__fett__ und _kursiv_")), &vec![Inline::Bold(vec![t("fett")]), t(" und "), Inline::Italic(vec![t("kursiv")])]);
    }

    #[test]
    fn emphasis_nesting_and_edges() {
        let d = p("*a **b** c*");
        assert_eq!(para(&d), &vec![Inline::Italic(vec![t("a "), Inline::Bold(vec![t("b")]), t(" c")])]);
        // Snake-Case bleibt unverändert
        assert_eq!(para(&p("snake_case_name")), &vec![t("snake_case_name")]);
        // nicht geschlossen
        assert_eq!(para(&p("**offen")), &vec![t("**offen")]);
        assert_eq!(para(&p("a * b * c")), &vec![t("a * b * c")]);
        // Escapes
        assert_eq!(para(&p(r"\*kein\* fett")), &vec![t("*kein* fett")]);
        // Kursiv mitten im Wort mit *
        assert_eq!(para(&p("un*glaub*lich")), &vec![t("un"), Inline::Italic(vec![t("glaub")]), t("lich")]);
    }

    #[test]
    fn code_spans() {
        assert_eq!(para(&p("`a*b*c`")), &vec![Inline::Code("a*b*c".into())]);
        assert_eq!(para(&p("``a`b``")), &vec![Inline::Code("a`b".into())]);
        assert_eq!(para(&p("`offen")), &vec![t("`offen")]);
        assert_eq!(para(&p("` x `")), &vec![Inline::Code("x".into())]);
    }

    #[test]
    fn code_blocks() {
        let d = p("```rust\nfn main() {}\n\n# kein Titel\n```\nnach");
        assert_eq!(d.blocks[0], Block::CodeBlock { lang: "rust".into(), code: "fn main() {}\n\n# kein Titel".into() });
        assert!(matches!(d.blocks[1], Block::Paragraph(_)));
        // nicht geschlossen: bis zum Ende
        let d = p("```\nabc\n- x");
        assert_eq!(d.blocks, vec![Block::CodeBlock { lang: "".into(), code: "abc\n- x".into() }]);
        // Tilden und längere Zäune
        let d = p("````\n```\ninnen\n```\n````");
        assert_eq!(d.blocks, vec![Block::CodeBlock { lang: "".into(), code: "```\ninnen\n```".into() }]);
        let d = p("~~~\nx\n~~~");
        assert_eq!(d.blocks, vec![Block::CodeBlock { lang: "".into(), code: "x".into() }]);
        // leerer Block
        assert_eq!(p("```\n```").blocks, vec![Block::CodeBlock { lang: "".into(), code: "".into() }]);
    }

    #[test]
    fn lists_flat() {
        let d = p("- a\n- b\n* c\n+ d");
        match &d.blocks[0] {
            Block::List { ordered, items, .. } => {
                assert!(!ordered);
                assert_eq!(items.len(), 4);
            }
            o => panic!("{o:?}"),
        }
        let d = p("1. a\n2. b\n3) c");
        match &d.blocks[0] {
            Block::List { ordered, start, items } => {
                assert!(ordered);
                assert_eq!(*start, 1);
                assert_eq!(items.len(), 3);
            }
            o => panic!("{o:?}"),
        }
        let d = p("5. fünf\n6. sechs");
        assert!(matches!(&d.blocks[0], Block::List { start: 5, .. }));
        // Liste und Absatz getrennt
        let d = p("- a\n\nAbsatz");
        assert_eq!(d.blocks.len(), 2);
    }

    #[test]
    fn lists_nested() {
        let d = p("- a\n  - b\n    - c\n- d\n  1. x\n  2. y");
        let Block::List { items, .. } = &d.blocks[0] else { panic!() };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].blocks.len(), 2);
        let Block::List { items: sub, .. } = &items[0].blocks[1] else { panic!() };
        assert_eq!(sub.len(), 1);
        assert!(matches!(&sub[0].blocks[1], Block::List { .. }));
        let Block::List { ordered, items: sub2, .. } = &items[1].blocks[1] else { panic!() };
        assert!(*ordered);
        assert_eq!(sub2.len(), 2);
        // zwei Leerzeichen unter nummerierten Einträgen genügen (großzügig)
        let d = p("1. a\n  - b\n2. c");
        let Block::List { items, .. } = &d.blocks[0] else { panic!() };
        assert_eq!(items.len(), 2);
        assert!(matches!(&items[0].blocks[1], Block::List { .. }));
    }

    #[test]
    fn list_continuation_and_paragraphs() {
        // weiche Fortsetzung
        let d = p("- eins\nzwei\n- drei");
        let Block::List { items, .. } = &d.blocks[0] else { panic!() };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].blocks, vec![Block::Paragraph(vec![t("eins"), Inline::SoftBreak, t("zwei")])]);
        // mehrere Absätze im Eintrag
        let d = p("- a\n\n  b\n- c");
        let Block::List { items, .. } = &d.blocks[0] else { panic!() };
        assert_eq!(items[0].blocks.len(), 2);
        // Leerzeile zwischen Einträgen hält die Liste zusammen
        let d = p("- a\n\n- b");
        assert!(matches!(&d.blocks[0], Block::List { items, .. } if items.len() == 2));
        assert_eq!(d.blocks.len(), 1);
        // leerer Eintrag
        assert!(matches!(&p("-").blocks[0], Block::List { items, .. } if items.len() == 1));
        // Liste unterbricht Absatz
        let d = p("Text\n- a");
        assert_eq!(d.blocks.len(), 2);
        // „2.“ unterbricht keinen Absatz
        let d = p("Text\n2. kein Listenstart");
        assert_eq!(d.blocks.len(), 1);
    }

    #[test]
    fn task_lists() {
        let d = p("- [ ] offen\n- [x] fertig\n- [X] auch\n- [y] nein\n- [ ]\n1. [ ] nummeriert");
        let Block::List { items, .. } = &d.blocks[0] else { panic!() };
        assert_eq!(items[0].task, Some(false));
        assert_eq!(items[1].task, Some(true));
        assert_eq!(items[2].task, Some(true));
        assert_eq!(items[3].task, None);
        assert_eq!(items[4].task, Some(false));
        assert_eq!(items[0].blocks, vec![Block::Paragraph(vec![t("offen")])]);
        // [y] bleibt Text
        assert_eq!(items[3].blocks, vec![Block::Paragraph(vec![t("[y] nein")])]);
        // nummerierte Aufgabe steht in eigener (nummerierter) Liste
        assert!(matches!(&d.blocks[1], Block::List { ordered: true, items, .. } if items[0].task == Some(false)));
        // verschachtelte Aufgaben
        let d = p("- [ ] a\n  - [x] b");
        let Block::List { items, .. } = &d.blocks[0] else { panic!() };
        let Block::List { items: sub, .. } = &items[0].blocks[1] else { panic!() };
        assert_eq!(sub[0].task, Some(true));
    }

    #[test]
    fn quotes() {
        let d = p("> a\n> b\n>\n> c\n\nnach");
        let Block::Quote(inner) = &d.blocks[0] else { panic!() };
        assert_eq!(inner.len(), 2);
        assert!(matches!(d.blocks[1], Block::Paragraph(_)));
        // verschachtelt, mit Liste darin
        let d = p("> - a\n> - b\n>> tief");
        let Block::Quote(inner) = &d.blocks[0] else { panic!() };
        assert!(matches!(inner[0], Block::List { .. }));
        assert!(matches!(inner[1], Block::Quote(_)));
        // Zitat ohne Leerzeichen
        assert!(matches!(&p(">x").blocks[0], Block::Quote(_)));
    }

    #[test]
    fn rules() {
        assert_eq!(p("---").blocks, vec![Block::Rule]);
        assert_eq!(p("***").blocks, vec![Block::Rule]);
        assert_eq!(p("* * *").blocks, vec![Block::Rule]);
        assert_eq!(p("_____").blocks, vec![Block::Rule]);
        assert_eq!(p("a\n\n---\n\nb").blocks.len(), 3);
        assert!(matches!(p("--").blocks[0], Block::Paragraph(_)));
        // nach Text ohne Leerzeile ebenfalls eine Linie
        assert_eq!(p("Text\n---").blocks.len(), 2);
    }

    #[test]
    fn tables_with_alignment() {
        let d = p("| A | B | C | D |\n|:--|:-:|--:|---|\n| 1 | 2 | 3 | 4 |\n| x | y |\n");
        let Block::Table { align, header, rows } = &d.blocks[0] else { panic!("{:?}", d.blocks) };
        assert_eq!(align, &vec![Align::Left, Align::Center, Align::Right, Align::None]);
        assert_eq!(header.len(), 4);
        assert_eq!(inline_text(&header[2]), "C");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].len(), 4); // aufgefüllt
        assert_eq!(inline_text(&rows[1][0]), "x");
        assert!(rows[1][3].is_empty());
    }

    #[test]
    fn tables_edge_cases() {
        // ohne äußere Pipes
        let d = p("a | b\n--- | ---\n1 | 2");
        assert!(matches!(&d.blocks[0], Block::Table { header, .. } if header.len() == 2));
        // Spaltenzahl passt nicht -> kein Tabellenkopf
        let d = p("a | b | c\n--- | ---\n1 | 2");
        assert!(matches!(d.blocks[0], Block::Paragraph(_)));
        // escapte Pipe und Inline-Formatierung in Zellen
        let d = p("| x |\n|---|\n| a \\| b **c** |");
        let Block::Table { rows, .. } = &d.blocks[0] else { panic!() };
        assert_eq!(rows[0][0], vec![t("a | b "), Inline::Bold(vec![t("c")])]);
        // Tabelle endet an Leerzeile
        let d = p("| x |\n|---|\n| 1 |\n\nAbsatz");
        assert_eq!(d.blocks.len(), 2);
        // nur Kopf ohne Zeilen
        let d = p("| x | y |\n|---|---|");
        assert!(matches!(&d.blocks[0], Block::Table { rows, .. } if rows.is_empty()));
        // Tabelle unterbricht einen Absatz
        let d = p("Text\n| x |\n|---|\n| 1 |");
        assert_eq!(d.blocks.len(), 2);
        assert!(matches!(d.blocks[1], Block::Table { .. }));
    }

    #[test]
    fn links() {
        let d = p("[Text](https://example.org/a_b) und [mehr **fett**](http://x.y \"Titel\")");
        let v = para(&d);
        assert_eq!(v[0], Inline::Link { text: vec![t("Text")], url: "https://example.org/a_b".into() });
        assert_eq!(v[2], Inline::Link { text: vec![t("mehr "), Inline::Bold(vec![t("fett")])], url: "http://x.y".into() });
        // kaputt: bleibt Text
        assert_eq!(para(&p("[a](b")), &vec![t("[a](b")]);
        assert_eq!(para(&p("[a] (b)")), &vec![t("[a] (b)")]);
        assert_eq!(para(&p("[a]")), &vec![t("[a]")]);
        // Klammern in der Adresse
        let d = p("[w](https://de.wikipedia.org/wiki/A_(B))");
        assert_eq!(para(&d)[0], Inline::Link { text: vec![t("w")], url: "https://de.wikipedia.org/wiki/A_(B)".into() });
        // leere Adresse
        assert!(matches!(&para(&p("[a]()"))[0], Inline::Link { url, .. } if url.is_empty()));
        // kein Link im Link
        let d = p("[a [b](c)](d)");
        assert!(matches!(&para(&d)[0], Inline::Link { .. }));
    }

    #[test]
    fn bare_urls_and_angle_links() {
        let d = p("Siehe https://example.org/x?a=1&b=2, danke.");
        let v = para(&d);
        assert_eq!(v[0], t("Siehe "));
        assert_eq!(v[1], Inline::Link { text: vec![t("https://example.org/x?a=1&b=2")], url: "https://example.org/x?a=1&b=2".into() });
        assert_eq!(v[2], t(", danke."));
        let d = p("(http://a.b/c) <https://d.e>");
        let v = para(&d);
        assert_eq!(v[1], Inline::Link { text: vec![t("http://a.b/c")], url: "http://a.b/c".into() });
        assert!(matches!(&v[3], Inline::Link { url, .. } if url == "https://d.e"));
        // nur das Schema reicht nicht
        assert_eq!(para(&p("http://")), &vec![t("http://")]);
        // Unterstriche in Adressen bleiben
        let d = p("http://a.b/foo_bar_baz");
        assert!(matches!(&para(&d)[0], Inline::Link { url, .. } if url == "http://a.b/foo_bar_baz"));
    }

    #[test]
    fn breaks() {
        assert_eq!(para(&p("a\nb")), &vec![t("a"), Inline::SoftBreak, t("b")]);
        assert_eq!(para(&p("a  \nb")), &vec![t("a"), Inline::HardBreak, t("b")]);
        assert_eq!(para(&p("a\\\nb")), &vec![t("a"), Inline::HardBreak, t("b")]);
        assert_eq!(para(&p("a \nb")), &vec![t("a"), Inline::SoftBreak, t("b")]);
        // CRLF und CR
        assert_eq!(para(&p("a\r\nb")), &vec![t("a"), Inline::SoftBreak, t("b")]);
        assert_eq!(para(&p("a\rb")), &vec![t("a"), Inline::SoftBreak, t("b")]);
        // Leerzeile trennt Absätze
        assert_eq!(p("a\n\nb").blocks.len(), 2);
    }

    #[test]
    fn ticket_refs_positive() {
        let d = pt("Siehe PAN-12, DEV-7. Und (PAN-3) sowie **PAN-4** und PAN-99!");
        let v = para(&d);
        let refs: Vec<&str> = v.iter().filter_map(|i| if let Inline::TicketRef(s) = i { Some(s.as_str()) } else { None }).collect();
        assert_eq!(refs, vec!["PAN-12", "DEV-7", "PAN-3", "PAN-99"]);
        // fett eingebettet
        assert!(v.iter().any(|i| matches!(i, Inline::Bold(c) if c == &vec![Inline::TicketRef("PAN-4".into())])));
        assert!(matches!(v.last(), Some(Inline::Text(s)) if s == "!"));
        // am Zeilenanfang, in Überschrift, Liste, Tabelle, Zitat
        assert!(matches!(&pt("PAN-1 geht").blocks[0], Block::Paragraph(v) if v[0] == Inline::TicketRef("PAN-1".into())));
        assert!(matches!(&pt("# PAN-1").blocks[0], Block::Heading { inlines, .. } if inlines[0] == Inline::TicketRef("PAN-1".into())));
        let d = pt("- PAN-5\n> DEV-6\n\n| a |\n|---|\n| PAN-8 |");
        let Block::List { items, .. } = &d.blocks[0] else { panic!() };
        assert_eq!(items[0].blocks, vec![Block::Paragraph(vec![Inline::TicketRef("PAN-5".into())])]);
        let Block::Table { rows, .. } = &d.blocks[2] else { panic!("{:?}", d.blocks) };
        assert_eq!(rows[0][0], vec![Inline::TicketRef("PAN-8".into())]);
        // genau 2 und 10 Buchstaben
        let long = |s: &str| parse(s, &|_| true);
        assert!(matches!(&para(&long("AB-1"))[0], Inline::TicketRef(_)));
        assert!(matches!(&para(&long("ABCDEFGHIJ-1"))[0], Inline::TicketRef(_)));
    }

    #[test]
    fn ticket_refs_negative() {
        let known = |s: &str| pt(s);
        let none = |d: &Doc| !format!("{d:?}").contains("TicketRef");
        assert!(none(&known("Code `PAN-12` hier")), "nicht in Code");
        assert!(none(&known("```\nPAN-12\n```")), "nicht im Codeblock");
        assert!(none(&known("https://x.org/PAN-12")), "nicht in Adresse");
        assert!(none(&known("[PAN-12](https://x.org)")), "nicht im Linktext");
        assert!(none(&known("[a](https://x.org/PAN-12)")), "nicht in Link-Ziel");
        assert!(none(&known("ABC-12")), "unbekanntes Kürzel");
        assert!(none(&known("pan-12")), "Kleinbuchstaben");
        assert!(none(&known("P-12")), "zu kurz");
        assert!(none(&known("ABCDEFGHIJK-1")), "zu lang (11)");
        assert!(none(&known("PAN-")), "ohne Zahl");
        assert!(none(&known("PAN12")), "ohne Bindestrich");
        assert!(none(&known("XPAN-12")), "Teil eines längeren Wortes");
        assert!(none(&known("aPAN-12")), "Wortmitte");
        assert!(none(&known("PAN-12abc")), "Buchstaben dahinter");
        assert!(none(&known("PAN-12.5")), "Versionsnummer");
        assert!(none(&known("PAN-12.txt")), "Dateiname");
        assert!(none(&known("PAN-12-3")), "weiterer Bindestrich");
        assert!(none(&known("PAN-1234567890")), "zu viele Ziffern");
        assert!(none(&known("/pfad/PAN-12")), "Pfad");
        assert!(none(&known("foo-PAN-12")), "nach Bindestrich");
        assert!(none(&parse_plain("PAN-12")), "ohne Kürzelliste nie");
    }

    #[test]
    fn robust_against_broken_input() {
        let cases = [
            "**", "*", "_", "__", "***", "****", "`", "``", "```", "[", "]", "[]", "[](", "[](]", "![", ">", ">>>>", "- ", "-", "1.", "1)", "|", "||", "|-|",
            "|a|\n|-|", "\\", "\\\n", "<", "<>", "<http", "http://", "- [", "- [ ", "#", "# ", "~~~", "***\n***", "> - [ ] > ```\n> x", "\t- a\n\t\t- b",
            "- a\n - b\n  - c\n   - d\n    - e",
        ];
        for c in cases {
            let _ = parse(c, &|_| true);
        }
        // sehr tief verschachtelt
        let deep_q = ">".repeat(10_000) + " x";
        let _ = p(&deep_q);
        let deep_l: String = (0..3000).map(|i| format!("{}- x\n", " ".repeat(i % 80))).collect();
        let _ = p(&deep_l);
        let deep_e = "*a ".repeat(20_000);
        let _ = p(&deep_e);
        let deep_e2 = "**a *b ".repeat(5_000) + &"* b** a*".repeat(5_000);
        let _ = p(&deep_e2);
        let br = "[".repeat(30_000);
        let _ = p(&br);
        let bt = "`a ".repeat(30_000);
        let _ = p(&bt);
        let tbl = "|".repeat(50_000);
        let _ = p(&tbl);
    }

    /// Einfacher Zufallsgenerator (xorshift), damit die Tests ohne Zusatzabhängigkeit streuen.
    pub(crate) struct Rng(pub u64);
    impl Rng {
        pub fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        pub fn below(&mut self, n: usize) -> usize {
            (self.next() % n.max(1) as u64) as usize
        }
    }

    pub(crate) fn random_markdown(rng: &mut Rng, len: usize) -> String {
        const PIECES: &[&str] = &[
            "*", "**", "_", "__", "`", "```", "~~~", "[", "]", "(", ")", "[x](y)", "![", "<", ">", "> ", "- ", "* ", "+ ", "1. ", "2) ", "- [ ] ", "- [x] ", "#", "## ", "|", " | ", "|---|", ":--:",
            "\n", "\n\n", "\n  ", "\n\t", "  \n", "\\", "\\\n", " ", "  ", "\t", "PAN-12", "DEV-1", "ABC-9", "http://a.b/c", "https://x.y", "---", "***", "a", "bc", "Hello", "ä", "€", "😀", "\u{0}",
            "\r\n", "\r", "[ ]", "9999999999.", "PAN-", "-1",
        ];
        let mut s = String::new();
        for _ in 0..len {
            s.push_str(PIECES[rng.below(PIECES.len())]);
        }
        s
    }

    #[test]
    fn fuzz_never_panics() {
        let mut seed = [0u8; 8];
        let _ = getrandom::fill(&mut seed);
        let random_seed = u64::from_le_bytes(seed) | 1;
        for base in [0x9E3779B97F4A7C15u64, 12345, random_seed] {
            let mut rng = Rng(base);
            for _ in 0..1500 {
                let len = rng.below(40);
                let s = random_markdown(&mut rng, len);
                let r = std::panic::catch_unwind(|| {
                    let _ = parse(&s, &|x| x == "PAN" || x == "DEV");
                });
                assert!(r.is_ok(), "Panik bei Seed {base}: {s:?}");
            }
        }
        // rohe Zufallszeichen
        let mut rng = Rng(random_seed);
        for _ in 0..500 {
            let len = rng.below(200);
            let s: String = (0..len).map(|_| char::from_u32(rng.below(0x3000) as u32).unwrap_or('?')).collect();
            let _ = parse(&s, &|_| true);
        }
    }
}
