//! Minimal `.editorconfig` support (https://spec.editorconfig.org): finds the files from the document's folder
//! upwards (until `root = true`), matches the section globs and collects the properties the editor uses.

use super::encoding::{LineEnding, TextEncoding};
use super::indent::IndentStyle;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Properties from `.editorconfig` that apply to one file. `None` = not set (or `unset`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditorConfig {
    /// `indent_style` + `indent_size` combined (spaces need a size; default 4 if only `indent_style = space`).
    pub indent_style: Option<IndentStyle>,
    pub tab_width: Option<usize>,
    pub end_of_line: Option<LineEnding>,
    pub charset: Option<TextEncoding>,
    pub trim_trailing_whitespace: Option<bool>,
    pub insert_final_newline: Option<bool>,
    /// The `.editorconfig` files that were read (closest last).
    pub files: Vec<PathBuf>,
}

impl EditorConfig {
    /// Nothing set?
    pub fn is_empty(&self) -> bool {
        self.indent_style.is_none()
            && self.tab_width.is_none()
            && self.end_of_line.is_none()
            && self.charset.is_none()
            && self.trim_trailing_whitespace.is_none()
            && self.insert_final_newline.is_none()
    }
}

/// Upper bound for folders walked upwards.
const MAX_LEVELS: usize = 64;
/// `.editorconfig` files larger than this are ignored.
const MAX_FILE_SIZE: u64 = 1024 * 1024;

/// Resolve the properties for the file at `path` (the file itself need not exist).
pub fn resolve(path: &Path) -> EditorConfig {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map(|d| d.join(path)).unwrap_or_else(|_| path.to_path_buf())
    };
    let mut found: Vec<(PathBuf, String)> = Vec::new();
    let mut dir = abs.parent();
    let mut levels = 0;
    while let Some(d) = dir {
        if levels >= MAX_LEVELS {
            break;
        }
        levels += 1;
        let f = d.join(".editorconfig");
        if let Ok(m) = std::fs::metadata(&f)
            && m.is_file()
            && m.len() <= MAX_FILE_SIZE
            && let Ok(bytes) = std::fs::read(&f)
        {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            let root = is_root(&text);
            found.push((f, text));
            if root {
                break;
            }
        }
        dir = d.parent();
    }
    let mut props: HashMap<String, String> = HashMap::new();
    let mut files = Vec::new();
    for (file, text) in found.iter().rev() {
        let base = file.parent().unwrap_or(Path::new(""));
        let rel = match abs.strip_prefix(base) {
            Ok(r) => to_slash(r),
            Err(_) => continue,
        };
        apply_file(text, &rel, &mut props);
        files.push(file.clone());
    }
    let mut ec = from_props(&props);
    ec.files = files;
    ec
}

/// Parse properties for `rel_path` (relative to the folder of the `.editorconfig`, `/`-separated) from one file's text.
pub fn parse_for(text: &str, rel_path: &str) -> EditorConfig {
    let mut props = HashMap::new();
    apply_file(text, rel_path, &mut props);
    from_props(&props)
}

fn to_slash(p: &Path) -> String {
    p.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/")
}

fn strip_bom(s: &str) -> &str {
    s.strip_prefix('\u{feff}').unwrap_or(s)
}

fn is_root(text: &str) -> bool {
    for line in strip_bom(text).lines() {
        let l = line.trim();
        if l.starts_with('[') {
            return false;
        }
        if let Some((k, v)) = l.split_once('=')
            && k.trim().eq_ignore_ascii_case("root")
            && v.trim().eq_ignore_ascii_case("true")
        {
            return true;
        }
    }
    false
}

fn apply_file(text: &str, rel_path: &str, props: &mut HashMap<String, String>) {
    let mut active = false;
    for line in strip_bom(text).lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') || l.starts_with(';') {
            continue;
        }
        if l.starts_with('[') && l.ends_with(']') && l.len() >= 2 {
            let glob = &l[1..l.len() - 1];
            active = glob_matches(glob, rel_path);
            continue;
        }
        if !active {
            continue;
        }
        if let Some((k, v)) = l.split_once('=') {
            let k = k.trim().to_ascii_lowercase();
            let v = v.trim().to_ascii_lowercase();
            if k.is_empty() {
                continue;
            }
            if v == "unset" {
                props.remove(&k);
            } else {
                props.insert(k, v);
            }
        }
    }
}

fn from_props(p: &HashMap<String, String>) -> EditorConfig {
    let get = |k: &str| p.get(k).map(|s| s.as_str());
    let num = |k: &str| get(k).and_then(|v| v.parse::<usize>().ok()).filter(|n| (1..=64).contains(n));
    let tab_width_prop = num("tab_width");
    let size_is_tab = get("indent_size") == Some("tab");
    let indent_size = num("indent_size");
    let indent_style = match get("indent_style") {
        Some("tab") => Some(IndentStyle::Tabs),
        Some("space") => Some(IndentStyle::Spaces(
            indent_size.or(if size_is_tab { tab_width_prop } else { None }).unwrap_or(4).min(16) as u8,
        )),
        _ => None,
    };
    // Spec: tab_width defaults to indent_size when that is a number.
    let tab_width = tab_width_prop.or(indent_size);
    let boolean = |k: &str| match get(k) {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    };
    EditorConfig {
        indent_style,
        tab_width,
        end_of_line: get("end_of_line").and_then(LineEnding::from_id),
        charset: get("charset").and_then(TextEncoding::from_id),
        trim_trailing_whitespace: boolean("trim_trailing_whitespace"),
        insert_final_newline: boolean("insert_final_newline"),
        files: Vec::new(),
    }
}

/// Does the section glob match `rel_path`?
pub fn glob_matches(glob: &str, rel_path: &str) -> bool {
    let (re, ranges) = glob_to_regex(glob);
    let Ok(rx) = regex::Regex::new(&re) else { return false };
    let Some(caps) = rx.captures(rel_path) else { return false };
    // Check numeric ranges `{a..b}` captured as groups.
    for (i, (lo, hi)) in ranges.iter().enumerate() {
        let Some(m) = caps.name(&format!("n{i}")) else { return false };
        match m.as_str().parse::<i64>() {
            Ok(n) if n >= *lo && n <= *hi => {}
            _ => return false,
        }
    }
    true
}

/// Translate an editorconfig glob into an anchored regex (plus the numeric ranges in capture order).
fn glob_to_regex(glob: &str) -> (String, Vec<(i64, i64)>) {
    let chars: Vec<char> = glob.chars().collect();
    let has_slash = {
        let mut in_class = false;
        let mut found = false;
        for c in &chars {
            match c {
                '[' => in_class = true,
                ']' => in_class = false,
                '/' if !in_class => found = true,
                _ => {}
            }
        }
        found
    };
    let mut start = 0;
    if chars.first() == Some(&'/') {
        start = 1;
    }
    let mut ranges = Vec::new();
    let body = convert(&chars[start..], &mut ranges);
    let re = if has_slash { format!("^{body}$") } else { format!("^(?:.*/)?{body}$") };
    (re, ranges)
}

fn find_matching_brace(c: &[char], open: usize) -> Option<usize> {
    let mut depth = 0;
    let mut i = open;
    while i < c.len() {
        match c[i] {
            '\\' => i += 1,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn convert(c: &[char], ranges: &mut Vec<(i64, i64)>) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        match ch {
            '\\' if i + 1 < c.len() => {
                out.push_str(&regex::escape(&c[i + 1].to_string()));
                i += 2;
                continue;
            }
            '*' => {
                if c.get(i + 1) == Some(&'*') {
                    out.push_str(".*");
                    i += 2;
                } else {
                    out.push_str("[^/]*");
                    i += 1;
                }
                continue;
            }
            '?' => out.push_str("[^/]"),
            '[' => {
                if let Some(close) = c[i + 1..].iter().position(|x| *x == ']').map(|p| p + i + 1) {
                    let inner = &c[i + 1..close];
                    let (neg, inner) = match inner.first() {
                        Some('!') | Some('^') => (true, &inner[1..]),
                        _ => (false, inner),
                    };
                    out.push('[');
                    if neg {
                        out.push('^');
                    }
                    for (k, x) in inner.iter().enumerate() {
                        if *x == '-' && k > 0 && k + 1 < inner.len() {
                            out.push('-');
                        } else if x.is_ascii_punctuation() && *x != '<' && *x != '>' {
                            // Any ASCII punctuation may be escaped in the regex syntax (`<`/`>` may not, but are
                            // not special inside a class).
                            out.push('\\');
                            out.push(*x);
                        } else {
                            out.push(*x);
                        }
                    }
                    out.push(']');
                    i = close + 1;
                    continue;
                }
                out.push_str("\\[");
            }
            '{' => {
                if let Some(close) = find_matching_brace(c, i) {
                    let inner: String = c[i + 1..close].iter().collect();
                    if let Some((a, b)) = numeric_range(&inner) {
                        out.push_str(&format!("(?P<n{}>[+-]?[0-9]+)", ranges.len()));
                        ranges.push((a.min(b), a.max(b)));
                        i = close + 1;
                        continue;
                    }
                    let parts = split_top_level(&c[i + 1..close]);
                    if parts.len() > 1 {
                        out.push_str("(?:");
                        let alts: Vec<String> = parts.iter().map(|p| convert(p, ranges)).collect();
                        out.push_str(&alts.join("|"));
                        out.push(')');
                        i = close + 1;
                        continue;
                    }
                }
                out.push_str("\\{");
            }
            _ => out.push_str(&regex::escape(&ch.to_string())),
        }
        i += 1;
    }
    out
}

fn numeric_range(s: &str) -> Option<(i64, i64)> {
    let (a, b) = s.split_once("..")?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

fn split_top_level(c: &[char]) -> Vec<&[char]> {
    let mut parts = Vec::new();
    let mut depth = 0;
    let mut last = 0;
    let mut i = 0;
    while i < c.len() {
        match c[i] {
            '\\' => i += 1,
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&c[last..i]);
                last = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    parts.push(&c[last..]);
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob_matches("*", "a.txt"));
        assert!(glob_matches("*", "sub/dir/a.txt"));
        assert!(glob_matches("*.rs", "src/main.rs"));
        assert!(!glob_matches("*.rs", "src/main.rsx"));
        assert!(glob_matches("*.{js,ts}", "a/b.ts"));
        assert!(!glob_matches("*.{js,ts}", "a/b.tsx"));
        assert!(glob_matches("src/*.rs", "src/a.rs"));
        assert!(!glob_matches("src/*.rs", "x/src/a.rs"));
        assert!(!glob_matches("src/*.rs", "src/sub/a.rs"));
        assert!(glob_matches("src/**.rs", "src/sub/a.rs"));
        assert!(glob_matches("/Makefile", "Makefile"));
        assert!(!glob_matches("/Makefile", "sub/Makefile"));
        assert!(glob_matches("Makefile", "sub/Makefile"));
        assert!(glob_matches("file[0-9].txt", "file5.txt"));
        assert!(!glob_matches("file[!0-9].txt", "file5.txt"));
        assert!(glob_matches("f?.md", "fx.md"));
        assert!(glob_matches("v{1..12}.txt", "v7.txt"));
        assert!(!glob_matches("v{1..12}.txt", "v13.txt"));
        assert!(glob_matches("{a,{b,c}}.x", "c.x"));
        assert!(glob_matches("{single}.x", "{single}.x"));
        assert!(glob_matches("a.(1)+", "a.(1)+"));
    }

    #[test]
    fn properties_and_overrides() {
        let text = "root = true\n\n[*]\nindent_style = space\nindent_size = 2\nend_of_line = LF\ncharset = utf-8\n\
                    trim_trailing_whitespace = true\ninsert_final_newline = true\n\n[*.go]\nindent_style = tab\n\
                    ; comment\n[Makefile]\nindent_style = tab\ntab_width = 8\n[*.md]\ntrim_trailing_whitespace = unset\n";
        let rs = parse_for(text, "src/a.rs");
        assert_eq!(rs.indent_style, Some(IndentStyle::Spaces(2)));
        assert_eq!(rs.tab_width, Some(2));
        assert_eq!(rs.end_of_line, Some(LineEnding::Lf));
        assert_eq!(rs.charset, Some(TextEncoding::Utf8));
        assert_eq!(rs.trim_trailing_whitespace, Some(true));
        assert_eq!(rs.insert_final_newline, Some(true));
        let go = parse_for(text, "main.go");
        assert_eq!(go.indent_style, Some(IndentStyle::Tabs));
        let mk = parse_for(text, "Makefile");
        assert_eq!(mk.tab_width, Some(8));
        let md = parse_for(text, "README.md");
        assert_eq!(md.trim_trailing_whitespace, None);
        assert!(parse_for("[*.py]\nindent_size=4", "a.rs").is_empty());
    }

    #[test]
    fn charset_and_indent_size_tab() {
        let ec = parse_for("[*]\ncharset = latin1\nindent_style = space\nindent_size = tab\ntab_width = 3\nend_of_line = crlf\n", "x");
        assert_eq!(ec.charset, Some(TextEncoding::Windows1252));
        assert_eq!(ec.indent_style, Some(IndentStyle::Spaces(3)));
        assert_eq!(ec.end_of_line, Some(LineEnding::CrLf));
        let ec = parse_for("[*]\ncharset = utf-8-bom\n", "x");
        assert_eq!(ec.charset, Some(TextEncoding::Utf8Bom));
    }

    #[test]
    fn resolve_walks_up_until_root() {
        let base = super::super::test_dir("ec");
        let sub = base.join("proj").join("src");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(base.join(".editorconfig"), "[*]\nindent_style = tab\ninsert_final_newline = false\n").unwrap();
        std::fs::write(base.join("proj").join(".editorconfig"), "root = true\n[*]\nindent_style = space\nindent_size = 3\n").unwrap();
        std::fs::write(sub.join(".editorconfig"), "[*.rs]\nend_of_line = crlf\n[src/*.rs]\ncharset = utf-16le\n").unwrap();
        let ec = resolve(&sub.join("main.rs"));
        assert_eq!(ec.indent_style, Some(IndentStyle::Spaces(3)));
        assert_eq!(ec.end_of_line, Some(LineEnding::CrLf));
        // `src/*.rs` is relative to the folder of that .editorconfig (src/), so it does not match src/main.rs.
        assert_eq!(ec.charset, None);
        // The outer file is above `root = true` and is ignored.
        assert_eq!(ec.insert_final_newline, None);
        assert_eq!(ec.files.len(), 2);
        let _ = std::fs::remove_dir_all(&base);
    }
}
