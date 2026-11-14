//! Formatierungsfunktionen der Werkzeugleiste: fügen Markdown-Zeichen an Auswahl/Cursor ein oder entfernen sie.
//!
//! Alle Positionen sind **Zeichenindizes** (nicht Bytes), wie sie auch `egui::text::CCursor` verwendet;
//! dadurch sind sie immer gültige Schnittstellen im Text. Zu große Werte werden begrenzt.

use std::ops::Range;

/// Formatierungen der Werkzeugleiste.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Bold,
    Italic,
    /// Inline-Code; bei mehrzeiliger Auswahl ein Codeblock.
    Code,
    /// Überschrift; mehrfaches Anwenden schaltet `#` → `##` → `###` → `####` → keine.
    Heading,
    List,
    NumberedList,
    TaskList,
    Quote,
    Link,
    /// Tabellengerüst mit Kopfzeile.
    Table,
}

/// Platzhaltertexte, die in den Text eingefügt werden (sprachabhängig, vom Aufrufer gewählt).
#[derive(Clone, Copy, Debug)]
pub struct Words {
    pub column: &'static str,
    pub link_text: &'static str,
}

impl Words {
    pub const EN: Words = Words { column: "Column", link_text: "link text" };
    pub const DE: Words = Words { column: "Spalte", link_text: "Linktext" };
}

/// Wendet `fmt` auf `text` an (englische Platzhalter). Siehe [`apply_format_with`].
pub fn apply_format(text: &str, selection: Range<usize>, fmt: Format) -> (String, Range<usize>) {
    apply_format_with(text, selection, fmt, Words::EN)
}

/// Wendet `fmt` auf `text` an. `selection` sind Zeichenindizes (leer = Cursor).
/// Gibt den neuen Text und die neue Auswahl zurück. Wiederholtes Anwenden schaltet (außer bei
/// Überschrift/Link/Tabelle) die Formatierung wieder aus.
pub fn apply_format_with(text: &str, selection: Range<usize>, fmt: Format, words: Words) -> (String, Range<usize>) {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let s = selection.start.min(n);
    let e = selection.end.min(n).max(s);
    let (out, sel) = match fmt {
        Format::Bold => inline(&chars, s, e, '*', 2),
        Format::Italic => inline(&chars, s, e, '*', 1),
        Format::Code => {
            if chars[s..e].contains(&'\n') {
                code_block(&chars, s, e)
            } else {
                inline(&chars, s, e, '`', 1)
            }
        }
        Format::Heading => lines(&chars, s, e, LineKind::Heading),
        Format::List => lines(&chars, s, e, LineKind::Bullet),
        Format::NumberedList => lines(&chars, s, e, LineKind::Ordered),
        Format::TaskList => lines(&chars, s, e, LineKind::Task),
        Format::Quote => lines(&chars, s, e, LineKind::Quote),
        Format::Link => link(&chars, s, e, words),
        Format::Table => table(&chars, s, e, words),
    };
    (out.into_iter().collect(), sel)
}

fn splice(chars: &[char], r: Range<usize>, with: &[char]) -> Vec<char> {
    let mut v = Vec::with_capacity(chars.len() + with.len());
    v.extend_from_slice(&chars[..r.start]);
    v.extend_from_slice(with);
    v.extend_from_slice(&chars[r.end..]);
    v
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

// ─────────────────────────── Inline ───────────────────────────

/// Länge der Folge von `c` in `chars[from..to]` ab `from` vorwärts.
fn run_fwd(chars: &[char], from: usize, to: usize, c: char) -> usize {
    chars[from..to].iter().take_while(|x| **x == c).count()
}

/// Länge der Folge von `c` in `chars[from..to]` rückwärts ab `to`.
fn run_back(chars: &[char], from: usize, to: usize, c: char) -> usize {
    chars[from..to].iter().rev().take_while(|x| **x == c).count()
}

/// Ist die Formatierung (Zeichen `c`, Breite `w`) in einer Folge der Länge `run` enthalten?
fn has_marker(c: char, w: usize, run: usize) -> bool {
    if c == '`' {
        return run >= 1;
    }
    if w >= 2 { run >= 2 } else { run % 2 == 1 }
}

/// Fett/Kursiv/Code um die Auswahl legen bzw. wieder entfernen.
fn inline(chars: &[char], s0: usize, e0: usize, c: char, w: usize) -> (Vec<char>, Range<usize>) {
    let n = chars.len();
    let (mut s, mut e) = (s0, e0);
    // Leerraum am Rand gehört nicht in die Auszeichnung („** x**“ wirkt nicht)
    while s < e && chars[s].is_whitespace() {
        s += 1;
    }
    while e > s && chars[e - 1].is_whitespace() {
        e -= 1;
    }
    if s == e {
        s = s0.min(e0);
        e = s;
        // Cursor an/in einem Wort: das Wort auszeichnen
        let mut a = s;
        let mut b = s;
        while a > 0 && is_word(chars[a - 1]) {
            a -= 1;
        }
        while b < n && is_word(chars[b]) {
            b += 1;
        }
        // nur wenn der Cursor nicht gerade zwischen zwei Markern steht
        let between = run_back(chars, 0, s, c) >= w && run_fwd(chars, s, n, c) >= w;
        if b > a && !between {
            s = a;
            e = b;
        }
    }
    let marker: Vec<char> = std::iter::repeat_n(c, w).collect();

    // A) Marker liegen innerhalb der Auswahl
    if e - s > 2 * w {
        let lead = run_fwd(chars, s, e, c);
        let trail = run_back(chars, s, e, c);
        if lead < e - s && has_marker(c, w, lead) && has_marker(c, w, trail) {
            let mut v = splice(chars, e - w..e, &[]);
            v = splice(&v, s..s + w, &[]);
            return (v, s..e - 2 * w);
        }
    }
    // B) Marker liegen unmittelbar um die Auswahl
    let before = run_back(chars, 0, s, c);
    let after = run_fwd(chars, e, n, c);
    if has_marker(c, w, before) && has_marker(c, w, after) {
        let mut v = splice(chars, e..e + w, &[]);
        v = splice(&v, s - w..s, &[]);
        return (v, s - w..e - w);
    }
    // C) umschließen
    let mut v = splice(chars, e..e, &marker);
    v = splice(&v, s..s, &marker);
    (v, s + w..e + w)
}

/// Mehrzeilige Auswahl als Codeblock.
fn code_block(chars: &[char], s: usize, e: usize) -> (Vec<char>, Range<usize>) {
    let sel: Vec<char> = chars[s..e].to_vec();
    // schon ein Block? („```\n…\n```“)
    let text: String = sel.iter().collect();
    let t = text.trim_matches('\n');
    if t.starts_with("```") && t.ends_with("```") && t.len() >= 8 {
        if let (Some(first_nl), Some(last_nl)) = (t.find('\n'), t.rfind('\n')) {
            if first_nl < last_nl {
                let inner: Vec<char> = t[first_nl + 1..last_nl].chars().collect();
                let v = splice(chars, s..e, &inner);
                return (v, s..s + inner.len());
            }
        }
    }
    let mut with: Vec<char> = Vec::new();
    let need_nl_before = s > 0 && chars[s - 1] != '\n';
    if need_nl_before {
        with.push('\n');
    }
    with.extend("```\n".chars());
    let inner_start = with.len();
    let mut body = sel.clone();
    while body.last() == Some(&'\n') {
        body.pop();
    }
    with.extend_from_slice(&body);
    let inner_end = with.len();
    with.extend("\n```".chars());
    if e < chars.len() && chars[e] != '\n' && !(e > s && chars[e - 1] == '\n') {
        with.push('\n');
    }
    let v = splice(chars, s..e, &with);
    (v, s + inner_start..s + inner_end)
}

// ─────────────────────────── Zeilen ───────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LineKind {
    Heading,
    Bullet,
    Ordered,
    Task,
    Quote,
}

/// Bestehendes Präfix einer Zeile (nach der Einrückung).
struct Prefix {
    indent: usize,
    len: usize,
    kind: Option<LineKind>,
    /// Überschriftstiefe (0 sonst).
    level: usize,
}

fn prefix_of(line: &[char]) -> Prefix {
    let indent = line.iter().take_while(|c| **c == ' ').count();
    let rest = &line[indent..];
    let starts = |p: &str| p.chars().enumerate().all(|(i, c)| rest.get(i) == Some(&c));
    // Überschrift
    let hashes = rest.iter().take_while(|c| **c == '#').count();
    if (1..=6).contains(&hashes) && (rest.len() == hashes || rest[hashes] == ' ') {
        return Prefix { indent, len: (hashes + 1).min(rest.len()), kind: Some(LineKind::Heading), level: hashes };
    }
    for b in ['-', '*', '+'] {
        for tag in [" [ ] ", " [x] ", " [X] "] {
            let p = format!("{b}{tag}");
            if starts(&p) {
                return Prefix { indent, len: p.chars().count(), kind: Some(LineKind::Task), level: 0 };
            }
        }
        for tag in [" [ ]", " [x]", " [X]"] {
            let p = format!("{b}{tag}");
            if rest.len() == p.chars().count() && starts(&p) {
                return Prefix { indent, len: rest.len(), kind: Some(LineKind::Task), level: 0 };
            }
        }
        if starts(&format!("{b} ")) || rest == [b] {
            return Prefix { indent, len: if rest.len() == 1 { 1 } else { 2 }, kind: Some(LineKind::Bullet), level: 0 };
        }
    }
    let digits = rest.iter().take_while(|c| c.is_ascii_digit()).count();
    if (1..=9).contains(&digits) && matches!(rest.get(digits), Some('.') | Some(')')) && (rest.len() == digits + 1 || rest.get(digits + 1) == Some(&' ')) {
        return Prefix { indent, len: (digits + 2).min(rest.len()), kind: Some(LineKind::Ordered), level: 0 };
    }
    if rest.first() == Some(&'>') {
        let len = if rest.get(1) == Some(&' ') { 2 } else { 1 };
        return Prefix { indent, len, kind: Some(LineKind::Quote), level: 0 };
    }
    Prefix { indent, len: 0, kind: None, level: 0 }
}

struct LineEdit {
    /// Zeilenanfang (alt) und Zeilenende (ohne Zeilenumbruch).
    start: usize,
    end: usize,
    indent: usize,
    old_len: usize,
    new_len: usize,
}

fn lines(chars: &[char], s: usize, e: usize, kind: LineKind) -> (Vec<char>, Range<usize>) {
    let n = chars.len();
    // betroffene Zeilen bestimmen
    let eff_end = if e > s && chars[e - 1] == '\n' { e - 1 } else { e };
    let mut first = s;
    while first > 0 && chars[first - 1] != '\n' {
        first -= 1;
    }
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut p = first;
    loop {
        let mut q = p;
        while q < n && chars[q] != '\n' {
            q += 1;
        }
        ranges.push((p, q));
        if q >= n || q >= eff_end {
            break;
        }
        p = q + 1;
        if p > eff_end {
            break;
        }
    }
    let multi = ranges.len() > 1;
    let infos: Vec<Prefix> = ranges.iter().map(|(a, b)| prefix_of(&chars[*a..*b])).collect();
    let active: Vec<usize> = (0..ranges.len())
        .filter(|i| !multi || chars[ranges[*i].0..ranges[*i].1].iter().any(|c| !c.is_whitespace()))
        .collect();
    // einschalten oder ausschalten?
    let all_have = !active.is_empty() && active.iter().all(|i| infos[*i].kind == Some(kind));
    let heading_level_next = {
        let l0 = active.first().map(|i| infos[*i].level).unwrap_or(0);
        if kind == LineKind::Heading {
            if l0 == 0 { 1 } else if l0 < 4 { l0 + 1 } else { 0 }
        } else {
            0
        }
    };
    let mut out: Vec<char> = Vec::with_capacity(n + 16);
    let mut edits: Vec<LineEdit> = Vec::new();
    let mut cursor = 0;
    let mut number = 1usize;
    for (idx, (a, b)) in ranges.iter().enumerate() {
        out.extend_from_slice(&chars[cursor..*a]);
        cursor = *b;
        let line = &chars[*a..*b];
        let info = &infos[idx];
        if !active.contains(&idx) {
            out.extend_from_slice(line);
            edits.push(LineEdit { start: *a, end: *b, indent: info.indent, old_len: 0, new_len: 0 });
            continue;
        }
        // neues Präfix
        let (new_prefix, strip_old): (String, bool) = match kind {
            LineKind::Quote => {
                if all_have {
                    (String::new(), true)
                } else {
                    ("> ".to_string(), false)
                }
            }
            LineKind::Heading => {
                if heading_level_next == 0 { (String::new(), true) } else { (format!("{} ", "#".repeat(heading_level_next)), true) }
            }
            LineKind::Bullet => {
                if all_have { (String::new(), true) } else { ("- ".to_string(), true) }
            }
            LineKind::Task => {
                if all_have { (String::new(), true) } else { ("- [ ] ".to_string(), true) }
            }
            LineKind::Ordered => {
                if all_have {
                    (String::new(), true)
                } else {
                    let p = format!("{number}. ");
                    number += 1;
                    (p, true)
                }
            }
        };
        // Quote ohne Strip behält ein vorhandenes anderes Präfix (z. B. Liste) im Text
        let (keep_from, old_len) = if strip_old || (kind == LineKind::Quote && all_have) { (info.indent + info.len, info.len) } else { (info.indent, 0) };
        let mut new_line: Vec<char> = line[..info.indent].to_vec();
        new_line.extend(new_prefix.chars());
        let new_len = new_prefix.chars().count();
        new_line.extend_from_slice(&line[keep_from..]);
        out.extend(new_line);
        edits.push(LineEdit { start: *a, end: *b, indent: info.indent, old_len, new_len });
    }
    out.extend_from_slice(&chars[cursor..]);
    let map = |pos: usize| -> usize {
        let mut shift: isize = 0;
        for ed in &edits {
            if pos < ed.start {
                break;
            }
            if pos <= ed.end {
                let o = pos - ed.start;
                let mapped = if o < ed.indent {
                    o
                } else if o <= ed.indent + ed.old_len {
                    ed.indent + ed.new_len
                } else {
                    o - ed.old_len + ed.new_len
                };
                return (ed.start as isize + shift + mapped as isize).max(0) as usize;
            }
            shift += ed.new_len as isize - ed.old_len as isize;
        }
        (pos as isize + shift).max(0) as usize
    };
    let (ns, ne) = (map(s), map(e));
    (out, ns..ne.max(ns))
}

// ─────────────────────────── Link / Tabelle ───────────────────────────

fn link(chars: &[char], s: usize, e: usize, w: Words) -> (Vec<char>, Range<usize>) {
    let sel: String = chars[s..e].iter().collect();
    let t = sel.trim();
    // vorhandener Link `[text](url)` -> nur Text
    if t.starts_with('[') && t.ends_with(')') {
        if let Some(mid) = t.find("](") {
            let text: Vec<char> = t[1..mid].chars().collect();
            let v = splice(chars, s..e, &text);
            return (v, s..s + text.len());
        }
    }
    let is_url = (t.starts_with("http://") || t.starts_with("https://")) && !t.contains(char::is_whitespace);
    let mut with: Vec<char> = Vec::new();
    let (a, b);
    if is_url {
        with.push('[');
        a = s + with.len();
        with.extend(w.link_text.chars());
        b = s + with.len();
        with.extend("](".chars());
        with.extend(t.chars());
        with.push(')');
    } else if t.is_empty() {
        with.push('[');
        a = s + with.len();
        with.extend(w.link_text.chars());
        b = s + with.len();
        with.extend("](https://)".chars());
    } else {
        with.push('[');
        with.extend(sel.chars());
        with.extend("](".chars());
        a = s + with.len();
        with.extend("https://".chars());
        b = s + with.len();
        with.push(')');
    }
    let v = splice(chars, s..e, &with);
    (v, a..b)
}

fn table(chars: &[char], s: usize, e: usize, w: Words) -> (Vec<char>, Range<usize>) {
    // hinter der Auswahl einfügen
    let at = e.max(s);
    let n = chars.len();
    let mut with = String::new();
    if at > 0 && chars[at - 1] != '\n' {
        with.push_str("\n\n");
    } else if at > 1 && chars[at - 2] != '\n' {
        with.push('\n');
    }
    let head_start = with.chars().count() + 2; // hinter „| “
    let c1 = format!("{} 1", w.column);
    let c2 = format!("{} 2", w.column);
    with.push_str(&format!("| {c1} | {c2} |\n| --- | --- |\n|  |  |"));
    if at >= n {
        with.push('\n');
    } else if chars[at] != '\n' {
        with.push_str("\n\n");
    } else if at + 1 < n && chars[at + 1] != '\n' {
        with.push('\n');
    }
    let ins: Vec<char> = with.chars().collect();
    let v = splice(chars, at..at, &ins);
    let a = at + head_start;
    (v, a..a + c1.chars().count())
}

// ─────────────────────────── Tests ───────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::parse::tests::Rng;

    fn f(text: &str, sel: Range<usize>, fmt: Format) -> (String, Range<usize>) {
        apply_format(text, sel, fmt)
    }

    /// Text mit Auswahlmarken `[` `]` → (Text, Bereich).
    fn mark(s: &str) -> (String, Range<usize>) {
        let a = s.chars().position(|c| c == '‹').unwrap();
        let b = s.chars().position(|c| c == '›').unwrap();
        let text: String = s.chars().filter(|c| *c != '‹' && *c != '›').collect();
        (text, a..b - 1)
    }

    fn show(text: &str, sel: Range<usize>) -> String {
        let mut v: Vec<char> = text.chars().collect();
        v.insert(sel.end, '›');
        v.insert(sel.start, '‹');
        v.into_iter().collect()
    }

    fn run(input: &str, fmt: Format) -> String {
        let (t, r) = mark(input);
        let (o, sel) = f(&t, r, fmt);
        show(&o, sel)
    }

    #[test]
    fn bold_wraps_and_unwraps() {
        assert_eq!(run("a ‹b› c", Format::Bold), "a **‹b›** c");
        assert_eq!(run("a **‹b›** c", Format::Bold), "a ‹b› c");
        // Auswahl samt Marker
        assert_eq!(run("a ‹**b**› c", Format::Bold), "a ‹b› c");
        // Leerraum am Rand bleibt außen
        assert_eq!(run("a‹ b ›c", Format::Bold), "a **‹b›** c");
    }

    #[test]
    fn bold_cursor_cases() {
        // Cursor im Wort: Wort wird fett
        assert_eq!(run("ab ‹›cd ef", Format::Bold), "ab **‹cd›** ef");
        assert_eq!(run("ab c‹›d ef", Format::Bold), "ab **‹cd›** ef");
        // Cursor zwischen Leerzeichen: leeres Paar, Cursor dazwischen
        assert_eq!(run("a ‹› b", Format::Bold), "a **‹›** b");
        // und wieder entfernen
        assert_eq!(run("a **‹›** b", Format::Bold), "a ‹› b");
        // leerer Text
        assert_eq!(run("‹›", Format::Bold), "**‹›**");
    }

    #[test]
    fn italic_and_code() {
        assert_eq!(run("‹x›", Format::Italic), "*‹x›*");
        assert_eq!(run("*‹x›*", Format::Italic), "‹x›");
        assert_eq!(run("‹x y›", Format::Code), "`‹x y›`");
        assert_eq!(run("`‹x y›`", Format::Code), "‹x y›");
        // Kursiv in Fett: Fett bleibt
        assert_eq!(run("**‹x›**", Format::Italic), "***‹x›***");
        assert_eq!(run("***‹x›***", Format::Italic), "**‹x›**");
        assert_eq!(run("***‹x›***", Format::Bold), "*‹x›*");
        // Fett erkennt Kursiv nicht als Fett
        assert_eq!(run("*‹x›*", Format::Bold), "***‹x›***");
    }

    #[test]
    fn roundtrips() {
        for fmt in [Format::Bold, Format::Italic, Format::Code, Format::List, Format::NumberedList, Format::TaskList, Format::Quote, Format::Link] {
            for input in ["‹abc›", "x ‹abc def› y", "‹äö€😀›", "‹›abc"] {
                let (t, r) = mark(input);
                let (o, r2) = f(&t, r.clone(), fmt);
                if fmt == Format::Link {
                    // Link: zweimal wirkt wie Aus-/Einschalten nur bei vollständiger Auswahl
                    continue;
                }
                let (o2, _) = f(&o, r2, fmt);
                assert_eq!(o2, t, "{fmt:?} auf {input:?}: {o:?}");
            }
        }
    }

    #[test]
    fn multiline_code_block() {
        let (t, r) = mark("a\n‹x\ny›\nb");
        let (o, sel) = f(&t, r, Format::Code);
        assert_eq!(o, "a\n```\nx\ny\n```\nb");
        assert_eq!(show(&o, sel), "a\n```\n‹x\ny›\n```\nb");
        // wieder zurück
        let (o2, _) = f(&o, 2..o.chars().count() - 2, Format::Code);
        assert!(o2.contains("x\ny") && !o2.contains("```"), "{o2:?}");
    }

    #[test]
    fn heading_cycles() {
        assert_eq!(run("‹abc›", Format::Heading), "# ‹abc›");
        assert_eq!(run("# ‹abc›", Format::Heading), "## ‹abc›");
        assert_eq!(run("### ‹abc›", Format::Heading), "#### ‹abc›");
        assert_eq!(run("#### ‹abc›", Format::Heading), "‹abc›");
        // Cursor am Zeilenanfang rückt hinter den Marker
        assert_eq!(run("‹›abc", Format::Heading), "# ‹›abc");
        // ersetzt Listenmarker
        assert_eq!(run("- ‹abc›", Format::Heading), "# ‹abc›");
        // zweite Zeile
        assert_eq!(run("x\n‹abc›", Format::Heading), "x\n# ‹abc›");
    }

    #[test]
    fn lists_toggle_and_convert() {
        assert_eq!(run("‹abc›", Format::List), "- ‹abc›");
        assert_eq!(run("- ‹abc›", Format::List), "‹abc›");
        assert_eq!(run("1. ‹abc›", Format::List), "- ‹abc›");
        assert_eq!(run("- [ ] ‹abc›", Format::List), "- ‹abc›");
        assert_eq!(run("‹a\nb\nc›", Format::List), "- ‹a\n- b\n- c›");
        assert_eq!(run("- ‹a\n- b›", Format::List), "‹a\nb›");
        // Leerzeile in der Mitte bleibt leer
        assert_eq!(run("‹a\n\nb›", Format::List), "- ‹a\n\n- b›");
        // eingerückt
        assert_eq!(run("  ‹a›", Format::List), "  - ‹a›");
        // Cursor auf leerer Zeile
        assert_eq!(run("a\n‹›\nb", Format::List), "a\n- ‹›\nb");
    }

    #[test]
    fn numbered_and_tasks() {
        assert_eq!(run("‹a\nb\nc›", Format::NumberedList), "1. ‹a\n2. b\n3. c›");
        assert_eq!(run("1. ‹a\n2. b›", Format::NumberedList), "‹a\nb›");
        assert_eq!(run("- ‹a\n- b›", Format::NumberedList), "1. ‹a\n2. b›");
        assert_eq!(run("‹a\nb›", Format::TaskList), "- [ ] ‹a\n- [ ] b›");
        assert_eq!(run("- [ ] ‹a›", Format::TaskList), "‹a›");
        assert_eq!(run("- [x] ‹a›", Format::TaskList), "‹a›");
        assert_eq!(run("- ‹a›", Format::TaskList), "- [ ] ‹a›");
    }

    #[test]
    fn quotes() {
        assert_eq!(run("‹a\nb›", Format::Quote), "> ‹a\n> b›");
        assert_eq!(run("> ‹a›", Format::Quote), "‹a›");
        // verschachtelt: ein zweites Zitat-Zeichen
        assert_eq!(run("- ‹a›", Format::Quote), "> - ‹a›");
        // Auswahl endet am Zeilenanfang der nächsten Zeile: diese bleibt unberührt
        let (o, _) = f("a\nb\nc", 0..2, Format::Quote);
        assert_eq!(o, "> a\nb\nc");
    }

    #[test]
    fn links() {
        assert_eq!(run("‹Text›", Format::Link), "[Text](‹https://›)");
        assert_eq!(run("‹https://x.y/z›", Format::Link), "[‹link text›](https://x.y/z)");
        assert_eq!(run("a ‹›b", Format::Link), "a [‹link text›](https://)b");
        assert_eq!(run("‹[Text](https://x.y)›", Format::Link), "‹Text›");
        let (o, _) = apply_format_with("", 0..0, Format::Link, Words::DE);
        assert_eq!(o, "[Linktext](https://)");
    }

    #[test]
    fn tables() {
        let (o, sel) = f("", 0..0, Format::Table);
        assert_eq!(o, "| Column 1 | Column 2 |\n| --- | --- |\n|  |  |\n");
        assert_eq!(o.chars().skip(sel.start).take(sel.len()).collect::<String>(), "Column 1");
        // mitten im Text: Leerzeile davor/danach
        let (o, sel) = f("Text", 4..4, Format::Table);
        assert!(o.starts_with("Text\n\n| Column 1"));
        assert_eq!(o.chars().skip(sel.start).take(sel.len()).collect::<String>(), "Column 1");
        let (o, _) = f("a\nb", 1..1, Format::Table);
        assert!(o.starts_with("a\n\n| Column 1"), "{o:?}");
        assert!(o.ends_with("|\n\nb"), "{o:?}");
        // der Parser erkennt das Gerüst
        let d = crate::markdown::parse::parse_plain(&o);
        assert!(d.blocks.iter().any(|b| matches!(b, crate::markdown::parse::Block::Table { .. })));
        let (o, _) = apply_format_with("", 0..0, Format::Table, Words::DE);
        assert!(o.contains("Spalte 1"));
    }

    #[test]
    fn unicode_and_bounds() {
        // Zeichenindizes, keine Bytes
        assert_eq!(run("äö ‹ü€›", Format::Bold), "äö **‹ü€›**");
        // Auswahl außerhalb: wird begrenzt
        let (o, sel) = f("abc", 10..20, Format::Bold);
        assert_eq!(o, "**abc**"); // Cursor am Wortende: Wort wird fett
        let (o2, sel2) = f("abc ", 10..20, Format::Bold);
        assert_eq!(o2, "abc ****");
        assert_eq!(sel2, 6..6);
        assert!(sel.end <= o.chars().count());
        let (o, _) = f("abc", 2..1, Format::Italic);
        assert!(!o.is_empty());
        let (o, _) = f("", 5..9, Format::Heading);
        assert_eq!(o, "# ");
    }

    #[test]
    fn fuzz_valid_selection_and_no_panic() {
        let mut rng = Rng(0xDEADBEEFCAFEF00D);
        let fmts = [
            Format::Bold,
            Format::Italic,
            Format::Code,
            Format::Heading,
            Format::List,
            Format::NumberedList,
            Format::TaskList,
            Format::Quote,
            Format::Link,
            Format::Table,
        ];
        for _ in 0..4000 {
            let len = rng.below(14);
            let text = crate::markdown::parse::tests::random_markdown(&mut rng, len);
            let n = text.chars().count();
            let a = rng.below(n + 3);
            let b = rng.below(n + 3);
            let fmt = fmts[rng.below(fmts.len())];
            let (out, sel) = apply_format(&text, a..b, fmt);
            let m = out.chars().count();
            assert!(sel.start <= sel.end && sel.end <= m, "{fmt:?} {text:?} {a}..{b} -> {out:?} {sel:?}");
        }
    }
}
