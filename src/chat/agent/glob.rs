//! Glob-Muster der Agenten-Werkzeuge (#338): `glob`, `grep --glob`, `.gitignore` und die Liste sensibler Dateien.
//!
//! `*` = beliebig viele Zeichen außer `/`, `**` als ganzer Abschnitt = beliebig viele Ordner (auch keiner), `?` = ein
//! Zeichen außer `/`, `[abc]`, `[a-z]`, `[!a]`/`[^a]` = Zeichenklasse, `{a,b}` = Alternativen. Pfade sind immer mit `/`
//! getrennt und relativ (z. B. `src/chat/agent/glob.rs`).

/// Höchstens so viele Muster entstehen beim Auflösen von `{a,b}` (mehr wird abgeschnitten).
const MAX_EXPANSIONS: usize = 64;
/// Längstes Muster (Zeichen); längere passen auf nichts.
pub const MAX_PATTERN: usize = 400;

/// `{a,b}` auflösen (auch verschachtelt): `src/{a,b}/*.rs` → `src/a/*.rs`, `src/b/*.rs`. Ohne Klammern: das Muster selbst.
pub fn expand_braces(p: &str) -> Vec<String> {
    let chars: Vec<char> = p.chars().collect();
    // Erste öffnende Klammer mit passender schließender (nicht in einer Zeichenklasse).
    let mut depth = 0usize;
    let mut open = None;
    let mut in_class = false;
    for (i, c) in chars.iter().enumerate() {
        match c {
            '[' if !in_class => in_class = true,
            ']' if in_class => in_class = false,
            '{' if !in_class => {
                if depth == 0 {
                    open = Some(i);
                }
                depth += 1;
            }
            '}' if !in_class && depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    let o = open.unwrap_or(0);
                    // Alternativen auf oberster Ebene der Klammer trennen.
                    let inner = &chars[o + 1..i];
                    let mut alts: Vec<String> = Vec::new();
                    let mut cur = String::new();
                    let mut d = 0usize;
                    for c in inner {
                        match c {
                            '{' => {
                                d += 1;
                                cur.push(*c);
                            }
                            '}' => {
                                d = d.saturating_sub(1);
                                cur.push(*c);
                            }
                            ',' if d == 0 => alts.push(std::mem::take(&mut cur)),
                            _ => cur.push(*c),
                        }
                    }
                    alts.push(cur);
                    let head: String = chars[..o].iter().collect();
                    let tail: String = chars[i + 1..].iter().collect();
                    let mut out = Vec::new();
                    for a in alts {
                        for e in expand_braces(&format!("{head}{a}{tail}")) {
                            if out.len() >= MAX_EXPANSIONS {
                                return out;
                            }
                            out.push(e);
                        }
                    }
                    return out;
                }
            }
            _ => {}
        }
    }
    vec![p.to_string()]
}

/// Passt der Pfad `path` auf das Muster? `ci` = Groß-/Kleinschreibung egal.
pub fn glob_match(pattern: &str, path: &str, ci: bool) -> bool {
    if pattern.chars().count() > MAX_PATTERN {
        return false;
    }
    expand_braces(pattern).iter().any(|p| match_one(p, path, ci))
}

fn match_one(pattern: &str, path: &str, ci: bool) -> bool {
    let ps: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let ts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    // Höchstens 8 `**`-Abschnitte (sonst wird das Zurücksetzen zu teuer).
    if ps.iter().filter(|s| **s == "**").count() > 8 {
        return false;
    }
    match_segs(&ps, &ts, ci)
}

fn match_segs(ps: &[&str], ts: &[&str], ci: bool) -> bool {
    let Some((first, rest)) = ps.split_first() else { return ts.is_empty() };
    if *first == "**" {
        // Folgende `**` zusammenfassen.
        let rest = {
            let mut r = rest;
            while r.first() == Some(&"**") {
                r = &r[1..];
            }
            r
        };
        return (0..=ts.len()).any(|k| match_segs(rest, &ts[k..], ci));
    }
    match ts.split_first() {
        Some((t, trest)) => seg_match(first, t, ci) && match_segs(rest, trest, ci),
        None => false,
    }
}

/// Ein Abschnitt (ohne `/`): `*`, `?`, `[…]`.
fn seg_match(p: &str, t: &str, ci: bool) -> bool {
    let norm = |s: &str| -> Vec<char> { if ci { s.chars().flat_map(char::to_lowercase).collect() } else { s.chars().collect() } };
    let p = norm(p);
    let t = norm(t);
    // Klassisches Zurücksetzen: Stelle des letzten `*` merken.
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() {
            match p[pi] {
                '*' => {
                    // `**` innerhalb eines Abschnitts wirkt wie `*`.
                    while pi < p.len() && p[pi] == '*' {
                        pi += 1;
                    }
                    star = Some((pi, ti));
                    continue;
                }
                '?' => {
                    pi += 1;
                    ti += 1;
                    continue;
                }
                '[' => {
                    if let Some((ok, len)) = class(&p[pi..], t[ti]) {
                        if ok {
                            pi += len;
                            ti += 1;
                            continue;
                        }
                    } else if t[ti] == '[' {
                        // Ohne schließende Klammer: ein wörtliches `[`.
                        pi += 1;
                        ti += 1;
                        continue;
                    }
                }
                '\\' if pi + 1 < p.len() => {
                    if p[pi + 1] == t[ti] {
                        pi += 2;
                        ti += 1;
                        continue;
                    }
                }
                c if c == t[ti] => {
                    pi += 1;
                    ti += 1;
                    continue;
                }
                _ => {}
            }
        }
        match star {
            Some((sp, st)) => {
                pi = sp;
                ti = st + 1;
                star = Some((sp, st + 1));
            }
            None => return false,
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Zeichenklasse am Anfang von `p` (`[…]`) gegen `c`: (passt, Länge der Klasse im Muster). `None` = keine gültige Klasse.
fn class(p: &[char], c: char) -> Option<(bool, usize)> {
    let mut i = 1;
    let neg = matches!(p.get(i), Some('!') | Some('^'));
    if neg {
        i += 1;
    }
    let start = i;
    let mut hit = false;
    while i < p.len() {
        if p[i] == ']' && i > start {
            return Some((hit != neg, i + 1));
        }
        let lo = p[i];
        if i + 2 < p.len() && p[i + 1] == '-' && p[i + 2] != ']' {
            let hi = p[i + 2];
            if lo <= c && c <= hi {
                hit = true;
            }
            i += 3;
        } else {
            if lo == c {
                hit = true;
            }
            i += 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stars_question_marks_and_classes() {
        assert!(glob_match("*.rs", "main.rs", false));
        assert!(!glob_match("*.rs", "src/main.rs", false), "* geht nicht über /");
        assert!(glob_match("src/*.rs", "src/main.rs", false));
        assert!(glob_match("src/**/*.rs", "src/main.rs", false), "** auch für keinen Ordner");
        assert!(glob_match("src/**/*.rs", "src/chat/agent/glob.rs", false));
        assert!(glob_match("**", "a/b/c", false));
        assert!(glob_match("**/target", "x/y/target", false));
        assert!(glob_match("a?c", "abc", false) && !glob_match("a?c", "a/c", false));
        assert!(glob_match("[a-c]x", "bx", false) && !glob_match("[!a-c]x", "bx", false) && glob_match("[^a-c]x", "dx", false));
        assert!(glob_match("file[.txt", "file[.txt", false), "offene Klammer wörtlich");
        assert!(glob_match("*.{rs,toml}", "Cargo.toml", false) && glob_match("*.{rs,toml}", "a.rs", false) && !glob_match("*.{rs,toml}", "a.md", false));
        assert!(glob_match("{src,tests}/**/*.rs", "tests/x.rs", false));
        assert!(glob_match("*.RS", "main.rs", true) && !glob_match("*.RS", "main.rs", false));
        assert!(glob_match(".env.*", ".env.local", false) && !glob_match(".env.*", ".env", false));
        assert!(glob_match("id_rsa*", "id_rsa.pub", false));
        assert!(glob_match("a*b*c", "aXXbYYc", false) && !glob_match("a*b*c", "aXXbYY", false));
        assert!(glob_match("\\*x", "*x", false) && !glob_match("\\*x", "ax", false));
    }

    #[test]
    fn braces_expand_nested_and_are_capped() {
        assert_eq!(expand_braces("a{b,c{d,e}}f"), ["abf", "acdf", "acef"]);
        assert_eq!(expand_braces("plain"), ["plain"]);
        assert_eq!(expand_braces("[{]x"), ["[{]x"], "Klammer in einer Zeichenklasse");
        let many = "{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}";
        assert_eq!(expand_braces(many).len(), MAX_EXPANSIONS);
        assert!(!glob_match(&"*".repeat(MAX_PATTERN + 1), "x", false));
        // viele ** werden abgelehnt statt lange zu suchen
        assert!(!glob_match("**/a/**/a/**/a/**/a/**/a/**/a/**/a/**/a/**/a/**/b", "a/a/a/a", false));
    }
}
