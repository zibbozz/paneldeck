//! Suchen im Projektordner (#338): Durchlauf mit `.gitignore`, `list_dir`, `glob` und `grep` (eingebaute Regex-Suche
//! mit `regex-automata`, ohne ripgrep).
//!
//! Der Durchlauf folgt keinen Symlinks/Junctions (sie werden gezählt, nicht betreten), überspringt `.git`, gesperrte
//! Ordner (`.ssh`, `.gnupg`) und – außer bei `include_ignored` – was `.gitignore`-Dateien im Projektordner oder
//! `.git/info/exclude` ausschließen. `grep` überspringt Binärdateien (NUL in den ersten 8 KB), Dateien über 4 MB und
//! sensible Dateien (die liest nur `read_file` mit Freigabe).

use super::glob::glob_match;
use super::paths::{self, Access, Resolved, Root};
use super::{err, int_arg, str_arg, AgentEnv, ToolErr, UNTRUSTED};
use regex_automata::meta::Regex;
use serde_json::{json, Value};
use std::path::Path;
use std::time::{Duration, Instant};

/// Höchstens so viele Einträge besucht ein Durchlauf.
pub const MAX_VISIT: usize = 100_000;
/// Höchstens so lange läuft ein Durchlauf.
pub const MAX_TIME: Duration = Duration::from_secs(15);
/// Größte Datei, die `grep` durchsucht.
pub const GREP_MAX_FILE: u64 = 4 << 20;
/// Höchstens so viele Treffer (`grep`) bzw. Pfade (`glob`, `list_dir`).
pub const MAX_MATCHES: usize = 200;
pub const MAX_PATHS: usize = 500;
/// Längste Trefferzeile (Zeichen).
const MAX_LINE_CHARS: usize = 300;

/// Groß-/Kleinschreibung in `.gitignore` egal (wie `core.ignorecase` unter Windows und macOS).
const IGNORE_CI: bool = cfg!(any(windows, target_os = "macos"));

/// Eine Regel einer `.gitignore`.
#[derive(Clone, Debug)]
struct Rule {
    pattern: String,
    negate: bool,
    dir_only: bool,
}

/// Regeln einer `.gitignore`-Datei samt ihrem Ordner (relativ zum Projektordner, `""` = Projektordner).
#[derive(Clone, Debug, Default)]
struct RuleSet {
    base: String,
    rules: Vec<Rule>,
}

fn parse_ignore(text: &str, base: &str) -> RuleSet {
    let mut rules = Vec::new();
    for raw in text.lines().take(5000) {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let line = line.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (negate, mut p) = match line.strip_prefix('!') {
            Some(rest) => (true, rest.to_string()),
            None => (false, line.strip_prefix('\\').filter(|r| r.starts_with('#') || r.starts_with('!')).unwrap_or(line).to_string()),
        };
        let dir_only = p.ends_with('/');
        while p.ends_with('/') {
            p.pop();
        }
        if p.is_empty() {
            continue;
        }
        let anchored = p.contains('/');
        let p = p.trim_start_matches('/').to_string();
        let pattern = if anchored { p } else { format!("**/{p}") };
        rules.push(Rule { pattern, negate, dir_only });
    }
    RuleSet { base: base.to_string(), rules }
}

/// Ist `rel` (relativ zum Projektordner) nach den Regeln ausgeschlossen? Die letzte passende Regel gilt.
fn ignored(sets: &[RuleSet], rel: &str, is_dir: bool) -> bool {
    let mut out = false;
    for s in sets {
        let sub = if s.base.is_empty() {
            rel
        } else {
            match rel.strip_prefix(&s.base).and_then(|r| r.strip_prefix('/')) {
                Some(r) => r,
                None => continue,
            }
        };
        for r in &s.rules {
            if r.dir_only && !is_dir {
                continue;
            }
            if glob_match(&r.pattern, sub, IGNORE_CI) {
                out = !r.negate;
            }
        }
    }
    out
}

fn load_ignore(dir: &Path, base: &str) -> Option<RuleSet> {
    let p = dir.join(".gitignore");
    let meta = std::fs::symlink_metadata(&p).ok()?;
    if !meta.is_file() || meta.len() > 1 << 20 {
        return None;
    }
    let text = std::fs::read_to_string(&p).ok()?;
    Some(parse_ignore(&text, base))
}

/// Ein Eintrag beim Durchlauf.
pub struct Item<'a> {
    /// Relativ zum Projektordner, mit `/`.
    pub rel: &'a str,
    pub is_dir: bool,
    /// Durch `.gitignore` ausgeschlossen (nur sichtbar mit `include_ignored`).
    pub ignored: bool,
    pub meta: Option<std::fs::Metadata>,
}

/// Zählerstand eines Durchlaufs.
#[derive(Default, Debug)]
pub struct Stats {
    pub visited: usize,
    pub links: usize,
    pub ignored: usize,
    /// Abgebrochen (Höchstzahl oder Zeit).
    pub stopped: bool,
}

pub struct WalkOpts {
    pub include_ignored: bool,
    /// Tiefe ab dem Startordner (1 = nur seine Einträge).
    pub max_depth: usize,
}

/// Rückgabe des Rückrufs: weiter oder aufhören.
pub enum Flow {
    Go,
    Stop,
}

/// Durchläuft `start` (ein Ordner im Projektordner) in Namensreihenfolge.
pub fn walk(root: &Root, start: &Resolved, opts: &WalkOpts, f: &mut dyn FnMut(Item<'_>) -> Flow) -> Stats {
    let mut st = Stats::default();
    let t0 = Instant::now();
    // Regeln vom Projektordner bis zum Startordner (alles innerhalb des Projekts).
    let mut sets: Vec<RuleSet> = Vec::new();
    if let Ok(t) = std::fs::read_to_string(root.canon.join(".git").join("info").join("exclude")) {
        sets.push(parse_ignore(&t, ""));
    }
    let mut cur = root.canon.clone();
    let mut base = String::new();
    if let Some(s) = load_ignore(&cur, "") {
        sets.push(s);
    }
    if start.rel != "." {
        for part in start.rel.split('/') {
            cur.push(part);
            base = if base.is_empty() { part.to_string() } else { format!("{base}/{part}") };
            if let Some(s) = load_ignore(&cur, &base) {
                sets.push(s);
            }
        }
    }
    let start_rel = if start.rel == "." { String::new() } else { start.rel.clone() };
    let mut stop = false;
    visit(&start.path, &start_rel, 1, opts, &mut sets, &mut st, t0, &mut stop, f);
    st.stopped |= stop && (st.visited >= MAX_VISIT || t0.elapsed() >= MAX_TIME);
    st
}

#[allow(clippy::too_many_arguments)]
fn visit(dir: &Path, rel: &str, depth: usize, opts: &WalkOpts, sets: &mut Vec<RuleSet>, st: &mut Stats, t0: Instant, stop: &mut bool, f: &mut dyn FnMut(Item<'_>) -> Flow) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<std::fs::DirEntry> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        if *stop {
            return;
        }
        st.visited += 1;
        if st.visited >= MAX_VISIT || (st.visited.is_multiple_of(256) &&t0.elapsed() >= MAX_TIME) {
            st.stopped = true;
            *stop = true;
            return;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            st.links += 1;
            continue;
        }
        let is_dir = ft.is_dir();
        if is_dir && (name == ".git" || matches!(name.to_lowercase().as_str(), ".ssh" | ".gnupg")) {
            continue;
        }
        let child_rel = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        let ign = ignored(sets, &child_rel, is_dir);
        if ign {
            st.ignored += 1;
            if !opts.include_ignored {
                continue;
            }
        }
        let path = e.path();
        let meta = e.metadata().ok();
        if let Flow::Stop = f(Item { rel: &child_rel, is_dir, ignored: ign, meta }) {
            *stop = true;
            return;
        }
        if is_dir && depth < opts.max_depth && depth < 64 {
            let pushed = load_ignore(&path, &child_rel).map(|s| sets.push(s)).is_some();
            visit(&path, &child_rel, depth + 1, opts, sets, st, t0, stop, f);
            if pushed {
                sets.pop();
            }
        }
    }
}

/// Startordner eines Suchwerkzeugs (`path`, Standard: Projektordner).
fn start_dir(root: &Root, env: &AgentEnv, args: &Value, tool: &str) -> Result<Resolved, ToolErr> {
    let p = str_arg(args, "path").unwrap_or(".");
    let r = paths::resolve(root, env, p, Access::Read)?;
    if !r.exists {
        return Err(paths::not_found(root, &r, "Folder", &format!("{{\"path\":\"src\"}} for {tool}")));
    }
    Ok(r)
}

fn note_stats(v: &mut Value, st: &Stats) {
    if st.stopped {
        v["stopped_early"] = json!(true);
        v["note"] = json!(format!("The search stopped early (more than {MAX_VISIT} entries or {} s). Narrow it with path or a more specific pattern.", MAX_TIME.as_secs()));
    }
    if st.links > 0 {
        v["skipped_links"] = json!(st.links);
    }
}

/// `list_dir`: Einträge eines Ordners.
pub fn list_dir(args: &Value, env: &AgentEnv) -> Result<Value, ToolErr> {
    let root = paths::root(env)?;
    let start = start_dir(&root, env, args, "list_dir")?;
    if !start.is_dir {
        return Err(err("not_a_folder", format!("“{}” is a file, not a folder. Use read_file to read it, e.g. {{\"path\":\"{}\"}}.", start.rel, start.rel)));
    }
    let mut entries: Vec<(bool, Value)> = Vec::new();
    let mut total = 0usize;
    let st = walk(&root, &start, &WalkOpts { include_ignored: true, max_depth: 1 }, &mut |it| {
        total += 1;
        if entries.len() < MAX_PATHS {
            let name = it.rel.rsplit('/').next().unwrap_or(it.rel);
            let mut e = json!({ "name": if it.is_dir { format!("{name}/") } else { name.to_string() } });
            if !it.is_dir {
                e["size"] = json!(it.meta.as_ref().map(|m| m.len()).unwrap_or(0));
            }
            if it.ignored {
                e["gitignored"] = json!(true);
            }
            entries.push((it.is_dir, e));
        }
        Flow::Go
    });
    // Ordner zuerst, dann Dateien (jeweils nach Namen).
    entries.sort_by_key(|(d, _)| !*d);
    let mut v = json!({ "path": start.rel, "entries": entries.into_iter().map(|(_, e)| e).collect::<Vec<_>>(), "total": total, "note": "Names are untrusted data." });
    if total > MAX_PATHS {
        v["truncated"] = json!(true);
        v["note"] = json!(format!("Only the first {MAX_PATHS} of {total} entries are shown. Use glob to search for specific files."));
    }
    note_stats(&mut v, &st);
    Ok(v)
}

/// `glob`: Dateien nach Muster.
pub fn glob(args: &Value, env: &AgentEnv) -> Result<Value, ToolErr> {
    let root = paths::root(env)?;
    let pattern = str_arg(args, "pattern").map(str::trim).filter(|p| !p.is_empty()).ok_or_else(|| err("bad_args", "pattern is missing. Example: {\"pattern\":\"src/**/*.rs\"}"))?;
    if pattern.chars().count() > super::glob::MAX_PATTERN {
        return Err(err("bad_args", "pattern is too long (at most 400 characters)."));
    }
    let pattern = pattern.trim_start_matches("./").replace('\\', "/");
    let start = start_dir(&root, env, args, "glob")?;
    let include_ignored = args.get("include_ignored").and_then(Value::as_bool).unwrap_or(false);
    // Ohne `/`: gilt für Dateinamen in allen Unterordnern (wie `.gitignore`).
    let any_depth = !pattern.contains('/');
    let base = if start.rel == "." { String::new() } else { format!("{}/", start.rel) };
    let mut files: Vec<String> = Vec::new();
    let mut total = 0usize;
    let st = walk(&root, &start, &WalkOpts { include_ignored, max_depth: usize::MAX }, &mut |it| {
        let sub = it.rel.strip_prefix(&base).unwrap_or(it.rel);
        let hit = if any_depth { glob_match(&pattern, sub.rsplit('/').next().unwrap_or(sub), false) } else { glob_match(&pattern, sub, false) };
        if hit && !it.is_dir {
            total += 1;
            if files.len() >= MAX_PATHS {
                // Genug: nicht weiter suchen (die Gesamtzahl bleibt dann offen).
                return Flow::Stop;
            }
            files.push(it.rel.to_string());
        }
        Flow::Go
    });
    let more = total > files.len();
    let mut v = json!({ "pattern": pattern, "files": files });
    if more {
        v["truncated"] = json!(true);
        v["note"] = json!(format!("More than {MAX_PATHS} files match; only the first {MAX_PATHS} are shown. Use a more specific pattern or path."));
    } else {
        v["total"] = json!(total);
    }
    if total == 0 {
        v["note"] = json!(if include_ignored || st.ignored == 0 {
            "No files match. Patterns: * = any name part, ** = any folders, e.g. src/**/*.rs or *.toml (without / it matches file names in all folders).".to_string()
        } else {
            format!("No files match ({} entries ignored by .gitignore were skipped; pass include_ignored=true to search them too).", st.ignored)
        });
    }
    note_stats(&mut v, &st);
    Ok(v)
}

/// Ausgabeart von `grep`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Content,
    Files,
    Count,
}

/// Datei als Text (oder `None` bei Binärdaten).
pub fn looks_binary(head: &[u8]) -> bool {
    head.iter().take(8192).any(|b| *b == 0)
}

/// `grep`: Regex-Suche in Dateien.
pub fn grep(args: &Value, env: &AgentEnv) -> Result<(Value, Vec<String>), ToolErr> {
    let root = paths::root(env)?;
    let mut fixes = Vec::new();
    let pattern = str_arg(args, "pattern").filter(|p| !p.is_empty()).ok_or_else(|| err("bad_args", "pattern is missing. Example: {\"pattern\":\"fn main\",\"glob\":\"*.rs\"}"))?;
    if pattern.len() > 1000 {
        return Err(err("bad_args", "pattern is too long (at most 1000 characters)."));
    }
    let ci = args.get("case_insensitive").and_then(Value::as_bool).unwrap_or(false);
    let re = Regex::builder()
        .syntax(regex_automata::util::syntax::Config::new().case_insensitive(ci).multi_line(true))
        .configure(Regex::config().nfa_size_limit(Some(8 << 20)))
        .build(pattern)
        .map_err(|e| {
            let lit = escape_regex(pattern);
            err("bad_regex", format!("The pattern is not a valid regular expression ({}). To search for the text literally, escape special characters, e.g. {{\"pattern\":{}}}.", crate::mcp::short(&e.to_string(), 300), json!(lit)))
        })?;
    let mode = match str_arg(args, "output_mode").unwrap_or("content") {
        "content" => Mode::Content,
        "files_with_matches" | "files" => Mode::Files,
        "count" => Mode::Count,
        other => return Err(err("bad_args", format!("output_mode must be content, files_with_matches or count, not “{}”. Example: {{\"pattern\":\"TODO\",\"output_mode\":\"files_with_matches\"}}", crate::mcp::short(other, 40)))),
    };
    let context = match int_arg(args, "context") {
        Some(c) if c > 5 => {
            fixes.push(format!("context {c} → 5 (maximum)"));
            5
        }
        Some(c) => c.max(0) as usize,
        None => 0,
    };
    let max = match int_arg(args, "max_results") {
        Some(m) if m > MAX_MATCHES as i64 => {
            fixes.push(format!("max_results {m} → {MAX_MATCHES} (maximum)"));
            MAX_MATCHES
        }
        Some(m) => m.max(1) as usize,
        None => MAX_MATCHES,
    };
    let file_glob = str_arg(args, "glob").map(|g| g.trim().trim_start_matches("./").replace('\\', "/")).filter(|g| !g.is_empty());
    let start = paths::resolve(&root, env, str_arg(args, "path").unwrap_or("."), Access::Read)?;
    if !start.exists {
        return Err(paths::not_found(&root, &start, "Path", "{\"pattern\":\"fn main\",\"path\":\"src\"}"));
    }
    let mut out: Vec<String> = Vec::new();
    let mut total = 0usize;
    let mut files_hit = 0usize;
    let (mut binary, mut large, mut sensitive, mut searched) = (0usize, 0usize, 0usize, 0usize);
    let mut search_file = |rel: &str, size: u64| -> Flow {
        if let Some(g) = &file_glob {
            let name = rel.rsplit('/').next().unwrap_or(rel);
            if !(glob_match(g, name, false) || glob_match(g, rel, false) || glob_match(&format!("**/{g}"), rel, false)) {
                return Flow::Go;
            }
        }
        if paths::is_sensitive(rel, &env.settings.sensitive) {
            sensitive += 1;
            return Flow::Go;
        }
        if size > GREP_MAX_FILE {
            large += 1;
            return Flow::Go;
        }
        let Ok(r) = paths::resolve(&root, env, rel, Access::Read) else { return Flow::Go };
        let Ok(f) = paths::open_read(&root, &r) else { return Flow::Go };
        let mut bytes = Vec::new();
        use std::io::Read;
        if f.take(GREP_MAX_FILE).read_to_end(&mut bytes).is_err() {
            return Flow::Go;
        }
        if looks_binary(&bytes) {
            binary += 1;
            return Flow::Go;
        }
        searched += 1;
        let mut lines: Vec<&[u8]> = bytes.split(|b| *b == b'\n').map(|l| l.strip_suffix(b"\r").unwrap_or(l)).collect();
        if bytes.ends_with(b"\n") {
            lines.pop();
        }
        let mut count = 0usize;
        let mut last_shown: Option<usize> = None;
        for (i, l) in lines.iter().enumerate() {
            if !re.is_match(*l) {
                continue;
            }
            count += 1;
            total += 1;
            if mode == Mode::Content && out.len() < max {
                let from = i.saturating_sub(context);
                let from = last_shown.map_or(from, |s| from.max(s + 1));
                if context > 0 && last_shown.is_some_and(|s| from > s + 1) {
                    out.push("--".into());
                }
                for (k, line) in lines.iter().enumerate().take(i + context + 1).skip(from) {
                    if k >= lines.len() || out.len() >= max {
                        break;
                    }
                    let sep = if k == i || re.is_match(*line) { ':' } else { '-' };
                    out.push(format!("{rel}{sep}{}{sep} {}", k + 1, clip_line(line)));
                    last_shown = Some(k);
                }
            }
        }
        if count > 0 {
            files_hit += 1;
            match mode {
                Mode::Files if out.len() < max => out.push(rel.to_string()),
                Mode::Count if out.len() < max => out.push(format!("{rel}: {count}")),
                _ => {}
            }
        }
        Flow::Go
    };
    let st = if start.is_dir {
        walk(&root, &start, &WalkOpts { include_ignored: false, max_depth: usize::MAX }, &mut |it| {
            if it.is_dir {
                return Flow::Go;
            }
            let size = it.meta.as_ref().map(|m| m.len()).unwrap_or(0);
            search_file(it.rel, size)
        })
    } else {
        let size = std::fs::metadata(&start.path).map(|m| m.len()).unwrap_or(0);
        search_file(&start.rel, size);
        Stats::default()
    };
    let key = match mode {
        Mode::Content => "matches",
        Mode::Files => "files",
        Mode::Count => "counts",
    };
    let shown_all = match mode {
        Mode::Content => total <= out.iter().filter(|l| l.as_str() != "--").count() || out.len() < max,
        _ => files_hit <= out.len(),
    };
    let mut v = json!({ "pattern": pattern, key: out, "total_matches": total, "files_with_matches": files_hit, "files_searched": searched, "untrusted_note": UNTRUSTED });
    let mut skipped = serde_json::Map::new();
    for (k, n) in [("binary", binary), ("larger_than_4mb", large), ("sensitive", sensitive), ("gitignored", st.ignored)] {
        if n > 0 {
            skipped.insert(k.into(), json!(n));
        }
    }
    if !skipped.is_empty() {
        v["skipped"] = Value::Object(skipped);
    }
    if sensitive > 0 {
        v["sensitive_note"] = json!("Sensitive files (e.g. .env, keys) are not searched; read_file can read them with the user's approval.");
    }
    if !shown_all || out.len() >= max && total > out.len() {
        v["truncated"] = json!(true);
        v["note"] = json!(format!("Only the first {} results are shown. Narrow the search with path, glob or a more specific pattern.", out.len()));
    } else if total == 0 {
        v["note"] = json!("No matches. Check the spelling, try case_insensitive=true or a shorter pattern.");
    }
    note_stats(&mut v, &st);
    Ok((v, fixes))
}

fn clip_line(l: &[u8]) -> String {
    let s = String::from_utf8_lossy(l);
    let s = s.trim_end();
    if s.chars().count() > MAX_LINE_CHARS {
        let cut: String = s.chars().take(MAX_LINE_CHARS).collect();
        format!("{cut}…")
    } else {
        s.to_string()
    }
}

/// Sonderzeichen einer Regex maskieren (für den Vorschlag „wörtlich suchen“).
pub fn escape_regex(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(c) {
            o.push('\\');
        }
        o.push(c);
    }
    o
}

#[cfg(test)]
mod tests {
    use super::super::paths::tests::tmp_root;
    use std::path::PathBuf;
    use super::*;

    fn tree(name: &str) -> (PathBuf, AgentEnv) {
        let (d, mut env) = tmp_root(name);
        env.settings.sensitive = paths::DEFAULT_SENSITIVE.iter().map(|s| s.to_string()).collect();
        for (p, c) in [
            ("src/main.rs", "fn main() {\n    println!(\"hello\");\n}\n"),
            ("src/chat/agent.rs", "// TODO: more\npub fn run() {}\nfn helper() { /* todo */ }\n"),
            ("README.md", "# Title\nTODO list\n"),
            ("target/debug/out.rs", "fn main() {} // generated\n"),
            ("logs/a.log", "TODO in a log\n"),
            ("logs/keep.log", "TODO kept\n"),
            (".env", "TOKEN=TODO-secret\n"),
            (".gitignore", "target/\n*.log\n!keep.log\n"),
            ("src/.gitignore", "/gen.rs\n"),
            ("src/gen.rs", "TODO generated\n"),
        ] {
            let path = d.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        }
        std::fs::write(d.join("src/bin.dat"), b"TODO\0\x01\x02").unwrap();
        std::fs::create_dir_all(d.join(".git")).unwrap();
        std::fs::write(d.join(".git/HEAD"), "TODO ref").unwrap();
        (d, env)
    }

    #[test]
    fn gitignore_rules_with_negation_anchors_and_dirs() {
        let s = vec![parse_ignore("target/\n*.log\n!keep.log\n# c\n\\#hash\n/only-root.txt\ndocs/*.tmp\n", "")];
        assert!(ignored(&s, "target", true) && !ignored(&s, "target", false), "nur Ordner");
        assert!(ignored(&s, "a/b/target", true));
        assert!(ignored(&s, "x.log", false) && ignored(&s, "deep/x.log", false));
        assert!(!ignored(&s, "keep.log", false), "Negation");
        assert!(ignored(&s, "#hash", false));
        assert!(ignored(&s, "only-root.txt", false) && !ignored(&s, "sub/only-root.txt", false), "verankert");
        assert!(ignored(&s, "docs/a.tmp", false) && !ignored(&s, "x/docs/a.tmp", false));
        let nested = vec![parse_ignore("/gen.rs\n", "src")];
        assert!(ignored(&nested, "src/gen.rs", false) && !ignored(&nested, "gen.rs", false) && !ignored(&nested, "src/x/gen.rs", false));
    }

    #[test]
    fn grep_respects_gitignore_binary_and_sensitive_files() {
        let (d, env) = tree("grep");
        let (v, fixes) = grep(&json!({"pattern": "TODO"}), &env).unwrap();
        assert!(fixes.is_empty());
        let m: Vec<String> = v["matches"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
        assert!(m.contains(&"README.md:2: TODO list".to_string()), "{m:?}");
        assert!(m.contains(&"src/chat/agent.rs:1: // TODO: more".to_string()), "{m:?}");
        assert!(m.contains(&"logs/keep.log:1: TODO kept".to_string()), "Negation in .gitignore: {m:?}");
        for not in ["target/", "logs/a.log", ".env", "src/gen.rs", "bin.dat", ".git/"] {
            assert!(!m.iter().any(|x| x.contains(not)), "{not}: {m:?}");
        }
        assert_eq!(v["skipped"]["binary"], 1);
        assert_eq!(v["skipped"]["sensitive"], 1);
        assert!(v["sensitive_note"].is_string());
        // Groß/klein, Dateiliste, Anzahl, glob, path
        let (v, _) = grep(&json!({"pattern": "todo", "case_insensitive": true, "output_mode": "files_with_matches"}), &env).unwrap();
        assert_eq!(v["files"], json!(["README.md", "logs/keep.log", "src/chat/agent.rs"]));
        let (v, _) = grep(&json!({"pattern": "todo", "case_insensitive": true, "output_mode": "count", "glob": "*.rs"}), &env).unwrap();
        assert_eq!(v["counts"], json!(["src/chat/agent.rs: 2"]));
        let (v, _) = grep(&json!({"pattern": "fn \\w+", "path": "src/main.rs"}), &env).unwrap();
        assert_eq!(v["matches"], json!(["src/main.rs:1: fn main() {"]));
        // Kontext und Korrekturen
        let (v, fixes) = grep(&json!({"pattern": "println", "context": 9, "max_results": 900}), &env).unwrap();
        assert_eq!(fixes, ["context 9 → 5 (maximum)", "max_results 900 → 200 (maximum)"]);
        assert_eq!(v["matches"], json!(["src/main.rs-1- fn main() {", "src/main.rs:2:     println!(\"hello\");", "src/main.rs-3- }"]));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn grep_errors_explain_and_suggest() {
        let (d, env) = tree("grep-err");
        let e = grep(&json!({"pattern": "fn main("}), &env).unwrap_err();
        assert_eq!(e.0, "bad_regex");
        assert!(e.1.contains("fn main\\\\("), "{}", e.1);
        let e = grep(&json!({"pattern": "x", "output_mode": "lines"}), &env).unwrap_err();
        assert!(e.1.contains("content, files_with_matches or count"), "{}", e.1);
        let e = grep(&json!({"pattern": "x", "path": "scr"}), &env).unwrap_err();
        assert!(e.1.contains("Did you mean src"), "{}", e.1);
        let (v, _) = grep(&json!({"pattern": "nothing-here-xyz"}), &env).unwrap();
        assert!(v["note"].as_str().unwrap().contains("No matches"));
        let (v, _) = grep(&json!({"pattern": "o", "max_results": 2}), &env).unwrap();
        assert_eq!(v["matches"].as_array().unwrap().len(), 2);
        assert_eq!(v["truncated"], true);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn glob_and_list_dir() {
        let (d, env) = tree("glob");
        let v = glob(&json!({"pattern": "src/**/*.rs"}), &env).unwrap();
        assert_eq!(v["files"], json!(["src/chat/agent.rs", "src/main.rs"]));
        let v = glob(&json!({"pattern": "*.rs"}), &env).unwrap();
        assert_eq!(v["files"], json!(["src/chat/agent.rs", "src/main.rs"]), "ohne / in allen Ordnern");
        let v = glob(&json!({"pattern": "*.rs", "include_ignored": true}), &env).unwrap();
        assert!(v["files"].as_array().unwrap().contains(&json!("target/debug/out.rs")));
        let v = glob(&json!({"pattern": "*.rs", "path": "src/chat"}), &env).unwrap();
        assert_eq!(v["files"], json!(["src/chat/agent.rs"]));
        let v = glob(&json!({"pattern": "*.xyz"}), &env).unwrap();
        assert!(v["note"].as_str().unwrap().contains("include_ignored"), "{v}");
        let v = list_dir(&json!({}), &env).unwrap();
        let names: Vec<&str> = v["entries"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["logs/", "src/", "target/", ".env", ".gitignore", "README.md"], "Ordner zuerst, .git fehlt");
        let t = v["entries"].as_array().unwrap().iter().find(|e| e["name"] == "target/").unwrap();
        assert_eq!(t["gitignored"], true);
        assert_eq!(v["entries"][5]["size"], 18);
        let e = list_dir(&json!({"path": "README.md"}), &env).unwrap_err();
        assert_eq!(e.0, "not_a_folder");
        let _ = std::fs::remove_dir_all(&d);
    }
}
