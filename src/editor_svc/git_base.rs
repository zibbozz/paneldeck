//! Git states of a file for the comparison and the change markers (#385, 3.2.0) – **only the local git CLI**, nothing
//! goes over the network. All functions block (git runs as a process, at most [`GIT_TIMEOUT`]); the editor calls them
//! from helper threads.
//!
//! Every call uses the hardened builder `gitops::command(.., read = true)`: no fsmonitor, no hooks, no network (all
//! protocols off, no lazy fetch of a partial clone), no credential helper, no signature check. Contents come from
//! `git cat-file --batch` without `--filters`/`--textconv`, so no filter or textconv driver of the repository runs; sizes
//! are checked first (`--batch-check`), large objects are never read. Refs typed by the user are checked
//! ([`check_ref`], git's ref-name rules) and verified (`rev-parse --verify`) before use.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Höchstdauer eines git-Aufrufs; danach wird er beendet und ein Fehler gemeldet.
pub const GIT_TIMEOUT: Duration = Duration::from_secs(10);

/// A file inside a git repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoFile {
    /// Root of the working tree.
    pub root: PathBuf,
    /// The repository folder (`.git`, or the folder of a worktree).
    pub git_dir: PathBuf,
    /// Path relative to the root, with `/`, in the case git knows it.
    pub rel: String,
}

/// Which state of the file.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Rev {
    Head,
    /// The index (staged state).
    Index,
    /// A commit (hash), a full ref name (`refs/heads/…`, `refs/tags/…`) or a typed revision (checked with
    /// [`check_ref`]).
    Commit(String),
}

impl Rev {
    /// Object name for `cat-file` (`HEAD:path`, `:path`, `<ref>:path`).
    fn spec(&self, rel: &str) -> String {
        match self {
            Rev::Head => format!("HEAD:{rel}"),
            Rev::Index => format!(":{rel}"),
            Rev::Commit(r) => format!("{r}:{rel}"),
        }
    }

    /// Short label (`HEAD`, `index`, branch/tag name without `refs/…/`, the first 7 characters of a hash).
    pub fn label(&self) -> String {
        match self {
            Rev::Head => "HEAD".into(),
            Rev::Index => "index".into(),
            Rev::Commit(r) => short_ref(r),
        }
    }

    /// Text for `editor-state.json` (`HEAD`, `:index`, ref).
    pub fn to_saved(&self) -> String {
        match self {
            Rev::Head => "HEAD".into(),
            Rev::Index => ":index".into(),
            Rev::Commit(r) => r.clone(),
        }
    }

    pub fn from_saved(s: &str) -> Option<Rev> {
        match s {
            "HEAD" => Some(Rev::Head),
            ":index" => Some(Rev::Index),
            r => check_ref(r).ok().map(|_| Rev::Commit(r.to_string())),
        }
    }
}

/// Anzeigename einer Revision: `refs/heads/x` → `x`, `refs/tags/v1` → `v1`, ein voller Hash → 7 Zeichen.
pub fn short_ref(r: &str) -> String {
    if let Some(s) = r.strip_prefix("refs/heads/").or_else(|| r.strip_prefix("refs/tags/")) {
        return s.to_string();
    }
    if r.len() == 40 && r.bytes().all(|b| b.is_ascii_hexdigit()) {
        return r[..7].to_string();
    }
    r.to_string()
}

/// Content of a state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Blob {
    Bytes(Vec<u8>),
    /// Not in this state (new file, not in the index, unknown commit).
    Missing,
    /// Larger than allowed (size in bytes).
    TooLarge(u64),
}

fn run_input(dir: &Path, args: &[&str], input: Option<Vec<u8>>) -> Result<Vec<u8>, String> {
    let mut cmd = crate::gitops::command(dir, true);
    cmd.args(args);
    let out = crate::gitops::output_timeout(cmd, input, GIT_TIMEOUT)?;
    if out.status.success() { Ok(out.stdout) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
}

fn run(dir: &Path, args: &[&str]) -> Result<String, String> {
    run_input(dir, args, None).map(|b| String::from_utf8_lossy(&b).to_string())
}

/// Is git there at all? (Tests überspringen ohne git.)
#[cfg(test)]
pub fn available() -> bool {
    run(Path::new("."), &["--version"]).is_ok()
}

/// Pfad ohne das Präfix `\\?\` von `canonicalize` unter Windows.
fn plain(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy().to_string();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => p,
    }
}

/// Repository of a file (`None`: not in a repository, or git missing). The relative path follows the real spelling of
/// the file (`SRC\File.txt` typed, `src/file.txt` on disk and in git).
pub fn locate(path: &Path) -> Option<RepoFile> {
    let path = std::fs::canonicalize(path).map(plain).unwrap_or_else(|_| path.to_path_buf());
    let parent = path.parent().filter(|p| p.is_dir())?;
    let name = path.file_name()?.to_string_lossy().to_string();
    let out = run(parent, &["rev-parse", "--show-toplevel", "--absolute-git-dir", "--show-prefix"]).ok()?;
    let mut lines = out.lines();
    let root = PathBuf::from(lines.next()?.trim());
    let git_dir = PathBuf::from(lines.next()?.trim());
    let prefix = lines.next().unwrap_or("").trim().to_string();
    if root.as_os_str().is_empty() {
        return None;
    }
    let mut rel = format!("{prefix}{name}");
    // Die Schreibweise im Index geht vor (Windows: Groß/Klein kann von der Platte abweichen).
    let exact = run(&root, &["ls-files", "--full-name", "-z", "--", &rel]).ok().filter(|o| o.split('\0').any(|s| s == rel));
    if exact.is_none() && cfg!(windows)
        && let Ok(o) = run(&root, &["ls-files", "--full-name", "-z"])
        && let Some(h) = o.split('\0').find(|s| s.eq_ignore_ascii_case(&rel))
    {
        // Selten: ein Index mit anderer Schreibweise als die Platte – den Eintrag ohne Groß/Klein nehmen.
        rel = h.to_string();
    }
    Some(RepoFile { root, git_dir, rel })
}

/// Eine Zeile von `cat-file --batch-check`/`--batch`: (Hash, Typ, Größe) oder `None` (`missing`, `ambiguous`; der Name
/// kann Leerzeichen enthalten).
fn parse_head(head: &str) -> Option<(String, String, u64)> {
    if head.ends_with(" missing") || head.ends_with(" ambiguous") {
        return None;
    }
    let mut parts: Vec<&str> = head.rsplitn(3, ' ').collect();
    parts.reverse();
    if parts.len() != 3 || !parts[2].bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((parts[0].to_string(), parts[1].to_string(), parts[2].parse().ok()?))
}

/// Contents of several states (each at most `max` bytes): first the sizes (`cat-file --batch-check`), then only the
/// blobs that fit (`cat-file --batch` by hash) – a huge object is never read.
pub fn blobs(repo: &RepoFile, revs: &[Rev], max: u64) -> Result<Vec<Blob>, String> {
    let mut input = String::new();
    for r in revs {
        // Zeilenumbrüche im Pfad würden die Anfrage teilen: so ein Pfad hat hier keinen Stand.
        if repo.rel.contains(['\n', '\r']) {
            input.push_str("invalid\u{1}path\n");
        } else {
            input.push_str(&r.spec(&repo.rel));
            input.push('\n');
        }
    }
    let check = run_input(&repo.root, &["cat-file", "--batch-check"], Some(input.into_bytes()))?;
    let check = String::from_utf8_lossy(&check).to_string();
    let heads: Vec<Option<(String, String, u64)>> = check.lines().map(parse_head).collect();
    let mut out: Vec<Blob> = Vec::with_capacity(revs.len());
    let mut want: Vec<(usize, String)> = Vec::new();
    for i in 0..revs.len() {
        match heads.get(i).cloned().flatten() {
            Some((sha, kind, size)) if kind == "blob" && size <= max => {
                want.push((i, sha));
                out.push(Blob::Missing);
            }
            Some((_, kind, size)) if kind == "blob" => out.push(Blob::TooLarge(size)),
            _ => out.push(Blob::Missing),
        }
    }
    if want.is_empty() {
        return Ok(out);
    }
    let input: String = want.iter().map(|(_, s)| format!("{s}\n")).collect();
    let data = run_input(&repo.root, &["cat-file", "--batch"], Some(input.into_bytes()))?;
    for ((i, _), b) in want.iter().zip(parse_batch(&data, want.len(), max)?) {
        out[*i] = b;
    }
    Ok(out)
}

/// Output of `cat-file --batch`: per object `<sha> <type> <size>\n<content>\n` or `<name> missing\n`.
fn parse_batch(out: &[u8], n: usize, max: u64) -> Result<Vec<Blob>, String> {
    let mut v = Vec::with_capacity(n);
    let mut pos = 0usize;
    while v.len() < n {
        let Some(nl) = out[pos..].iter().position(|b| *b == b'\n') else { break };
        let head = String::from_utf8_lossy(&out[pos..pos + nl]).to_string();
        pos += nl + 1;
        match parse_head(&head) {
            Some((_, kind, size)) => {
                let end = (pos + size as usize).min(out.len());
                let body = &out[pos..end];
                pos = (end + 1).min(out.len());
                v.push(if kind != "blob" {
                    Blob::Missing
                } else if size > max {
                    Blob::TooLarge(size)
                } else {
                    Blob::Bytes(body.to_vec())
                });
            }
            None => v.push(Blob::Missing),
        }
    }
    while v.len() < n {
        v.push(Blob::Missing);
    }
    Ok(v)
}

/// Is `r` a revision the user may type (branch, tag, commit, `HEAD~2`, `main^`, also `feat+x`, `#12`, `grün`)? Git's
/// ref-name rules: no leading `-` (never an option), no `..` (no ranges), no whitespace or control characters, none of
/// `: ? * [ \`, no `@{`, no `//`, not ending in `/`, `.` or `.lock`. `~` and `^` are allowed (revision suffixes).
pub fn check_ref(r: &str) -> Result<(), String> {
    let bad = |c: char| c.is_control() || c.is_whitespace() || matches!(c, ':' | '?' | '*' | '[' | '\\');
    let ok = !r.is_empty()
        && r.chars().count() <= 255
        && !r.starts_with('-')
        && !r.starts_with('/')
        && !r.contains("..")
        && !r.contains("@{")
        && !r.contains("//")
        && !r.ends_with('.')
        && !r.ends_with('/')
        && !r.ends_with(".lock")
        && !r.chars().any(bad);
    if ok { Ok(()) } else { Err(crate::state::t("Ungültige Revision (erlaubt: Zweig, Tag, Commit, z. B. main, v1.2, HEAD~2, a1b2c3d)", "Invalid revision (allowed: branch, tag, commit, e.g. main, v1.2, HEAD~2, a1b2c3d)").to_string()) }
}

/// Check and resolve a ref to its commit (full hash).
pub fn verify_ref(root: &Path, r: &str) -> Result<String, String> {
    check_ref(r)?;
    let spec = format!("{r}^{{commit}}");
    match run(root, &["rev-parse", "--verify", "--quiet", &spec]) {
        Ok(h) if !h.trim().is_empty() => Ok(h.trim().to_string()),
        _ => Err(crate::state::t("Diese Revision gibt es im Repository nicht.", "This revision does not exist in the repository.").to_string()),
    }
}

/// A branch or tag: full ref name (used for comparing) and the short name (shown).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefName {
    pub full: String,
    pub short: String,
}

/// Branches and tags, at most 300 each.
pub fn refs(root: &Path) -> (Vec<RefName>, Vec<RefName>) {
    let out = run(root, &["for-each-ref", "--format=%(refname)", "--count=600", "refs/heads", "refs/tags"]).unwrap_or_default();
    let mut branches = Vec::new();
    let mut tags = Vec::new();
    for l in out.lines().filter(|l| check_ref(l).is_ok()) {
        let r = RefName { full: l.to_string(), short: short_ref(l) };
        if l.starts_with("refs/heads/") {
            if branches.len() < 300 {
                branches.push(r);
            }
        } else if l.starts_with("refs/tags/") && tags.len() < 300 {
            tags.push(r);
        }
    }
    (branches, tags)
}

/// A commit of the file's history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub short: String,
    pub date: String,
    pub author: String,
    pub subject: String,
}

/// The last `n` commits that changed the file (no signature check: `gpg.program` never runs).
pub fn file_log(repo: &RepoFile, n: usize) -> Vec<Commit> {
    let count = format!("-n{}", n.clamp(1, 500));
    let out = run(&repo.root, &["log", &count, "--no-show-signature", "--no-color", "--date=short", "--format=%H%x1f%h%x1f%ad%x1f%an%x1f%s", "--", &repo.rel]).unwrap_or_default();
    out.lines()
        .filter_map(|l| {
            let p: Vec<&str> = l.split('\u{1f}').collect();
            (p.len() == 5).then(|| Commit { hash: p[0].into(), short: p[1].into(), date: p[2].into(), author: p[3].into(), subject: p[4].chars().take(200).collect() })
        })
        .collect()
}

/// Einen Stand in git dekodieren: Reiner ASCII-Inhalt nimmt die Kodierung des Dokuments (sonst zeigte die Hinweisleiste
/// bei einer Windows-1252-Datei fälschlich „UTF-8 ↔ Windows-1252“), sonst wird erkannt.
pub fn decode_like(bytes: &[u8], doc: Option<crate::editor::TextEncoding>) -> crate::editor::encoding::Decoded {
    use crate::editor::TextEncoding;
    let ascii = bytes.iter().all(|b| *b < 0x80);
    match doc {
        Some(e) if ascii && !matches!(e, TextEncoding::Utf16Le | TextEncoding::Utf16Be | TextEncoding::Utf8Bom) => crate::editor::encoding::decode(bytes, Some(e)),
        _ => crate::editor::encoding::decode(bytes, None),
    }
}
/// Stamp of the repository state that changes the comparison: index, HEAD and the log of HEAD (commit, checkout,
/// reset, staging). Cheap (three `stat` calls); a different stamp means: read the states again.
pub fn stamp(git_dir: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for f in ["index", "HEAD", "logs/HEAD"] {
        match std::fs::metadata(git_dir.join(f)) {
            Ok(m) => (m.len(), m.modified().ok()).hash(&mut h),
            Err(_) => 0u8.hash(&mut h),
        }
    }
    h.finish()
}

/// Stamp of a file on disk (size and time).
pub fn file_stamp(path: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    match std::fs::metadata(path) {
        Ok(m) => (m.len(), m.modified().ok()).hash(&mut h),
        Err(_) => 1u8.hash(&mut h),
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let root = std::env::var_os("PANELDECK_TEST_ROOT").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let d = root.join(format!("pd-gitbase-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn git(dir: &Path, args: &[&str]) {
        let st = crate::gitops::command(dir, false).args(["-c", "user.name=Test", "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false"]).args(args).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().unwrap();
        assert!(st.success(), "git {args:?}");
    }

    #[test]
    fn refs_are_checked_before_use() {
        for ok in ["main", "feature/x-1", "v1.2.3", "HEAD~2", "main^", "a1b2c3d", "@", "refs/heads/feat+x", "refs/tags/#12", "grün", "x;rm"] {
            assert!(check_ref(ok).is_ok(), "{ok}");
        }
        for bad in ["", "-n", "--all", "a..b", "a b", "a\nb", "ref.lock", "x.", "a/", "/a", "a//b", "main@{1}", "x?", "a*", "a[1", "a\\b", &"a".repeat(256), "a:b"] {
            assert!(check_ref(bad).is_err(), "{bad:?}");
        }
        assert_eq!(Rev::Commit("refs/heads/main".into()).label(), "main");
        assert_eq!(Rev::Commit("a".repeat(40)).label(), "aaaaaaa");
        assert_eq!(Rev::from_saved("refs/tags/v1"), Some(Rev::Commit("refs/tags/v1".into())));
        assert_eq!(Rev::from_saved("HEAD"), Some(Rev::Head));
        assert_eq!(Rev::from_saved(":index"), Some(Rev::Index));
        assert_eq!(Rev::from_saved("v1"), Some(Rev::Commit("v1".into())));
        assert_eq!(Rev::from_saved("--x"), None);
        for r in [Rev::Head, Rev::Index, Rev::Commit("main".into())] {
            assert_eq!(Rev::from_saved(&r.to_saved()), Some(r));
        }
    }

    #[test]
    fn batch_output_is_parsed() {
        let out = b"abc blob 5\nhello\nHEAD:x missing\ndef blob 3\nabc\n";
        assert_eq!(parse_batch(out, 3, 100).unwrap(), vec![Blob::Bytes(b"hello".to_vec()), Blob::Missing, Blob::Bytes(b"abc".to_vec())]);
        assert_eq!(parse_batch(out, 1, 2).unwrap(), vec![Blob::TooLarge(5)]);
        assert_eq!(parse_batch(b"", 2, 10).unwrap(), vec![Blob::Missing, Blob::Missing]);
        assert_eq!(parse_head("HEAD:a b.txt missing"), None);
        assert_eq!(parse_head("abc blob 12"), Some(("abc".into(), "blob".into(), 12)));
    }

    /// Lesen aus einem echten Repository (übersprungen ohne git).
    #[test]
    fn reads_head_index_refs_and_log_from_a_temp_repo() {
        if !available() {
            eprintln!("git fehlt – Test übersprungen");
            return;
        }
        let d = tmp("repo");
        git(&d, &["init", "-q", "-b", "main"]);
        std::fs::create_dir_all(d.join("src")).unwrap();
        let f = d.join("src").join("a b.txt");
        std::fs::write(&f, "one\ntwo\n").unwrap();
        git(&d, &["add", "-A"]);
        git(&d, &["commit", "-q", "-m", "first"]);
        git(&d, &["tag", "v1"]);
        std::fs::write(&f, "one\nTWO\n").unwrap();
        git(&d, &["add", "-A"]);
        std::fs::write(&f, "one\nTWO\nthree\n").unwrap();
        let st0 = stamp(&locate(&f).unwrap().git_dir);
        let r = locate(&f).expect("im Repository");
        assert_eq!(r.rel, "src/a b.txt");
        let b = blobs(&r, &[Rev::Index, Rev::Head, Rev::Commit("v1".into()), Rev::Commit("nope".into())], 1 << 20).unwrap();
        assert_eq!(b[0], Blob::Bytes(b"one\nTWO\n".to_vec()));
        assert_eq!(b[1], Blob::Bytes(b"one\ntwo\n".to_vec()));
        assert_eq!(b[2], Blob::Bytes(b"one\ntwo\n".to_vec()));
        assert_eq!(b[3], Blob::Missing);
        assert!(matches!(blobs(&r, &[Rev::Head], 3).unwrap()[0], Blob::TooLarge(8)));
        let (br, tags) = refs(&d);
        assert_eq!(br, vec![RefName { full: "refs/heads/main".into(), short: "main".into() }]);
        assert_eq!(tags, vec![RefName { full: "refs/tags/v1".into(), short: "v1".into() }]);
        assert_eq!(blobs(&r, &[Rev::Commit("refs/tags/v1".into())], 1 << 20).unwrap(), vec![Blob::Bytes(b"one\ntwo\n".to_vec())]);
        // Andere Schreibweise des Pfads (Windows: gleiche Datei)
        if cfg!(windows) {
            let upper = d.join("SRC").join("A B.TXT");
            assert_eq!(locate(&upper).unwrap().rel, "src/a b.txt");
        }
        // Signaturprüfung startet kein Programm (log.showSignature in der Konfiguration des Repositorys)
        let marker = d.join("gpg-ran");
        let fake = if cfg!(windows) { d.join("gpg.bat") } else { d.join("gpg.sh") };
        std::fs::write(&fake, if cfg!(windows) { format!("echo x > \"{}\"\r\n", marker.display()) } else { format!("#!/bin/sh\ntouch '{}'\n", marker.display()) }).unwrap();
        git(&d, &["config", "log.showSignature", "true"]);
        git(&d, &["config", "gpg.program", &fake.to_string_lossy()]);
        assert_eq!(file_log(&r, 5).len(), 1);
        assert!(!marker.exists(), "gpg.program lief");
        // Partial Clone mit `ext::`-Quelle: Lesen eines fehlenden Objekts lädt nichts nach und startet nichts
        let marker2 = d.join("ext-ran");
        git(&d, &["config", "remote.evil.url", &format!("ext::sh -c touch% {}", marker2.display())]);
        git(&d, &["config", "remote.evil.promisor", "true"]);
        git(&d, &["config", "extensions.partialClone", "evil"]);
        git(&d, &["config", "protocol.ext.allow", "always"]);
        let _ = blobs(&r, &[Rev::Commit("0123456789012345678901234567890123456789".into())], 1 << 20);
        assert!(!marker2.exists(), "ext:: lief");
        assert!(verify_ref(&d, "v1").unwrap().len() >= 40);
        assert!(verify_ref(&d, "nope").is_err());
        assert!(verify_ref(&d, "--all").is_err());
        let log = file_log(&r, 50);
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].subject, "first");
        // Neue Datei: weder im Index noch in HEAD
        let n = d.join("new.txt");
        std::fs::write(&n, "x").unwrap();
        let rn = locate(&n).unwrap();
        assert_eq!(blobs(&rn, &[Rev::Index, Rev::Head], 1 << 20).unwrap(), vec![Blob::Missing, Blob::Missing]);
        // Stempel ändert sich mit dem Index
        git(&d, &["add", "-A"]);
        assert_ne!(stamp(&r.git_dir), st0);
        // Außerhalb eines Repositorys
        let o = tmp("plain");
        std::fs::write(o.join("x.txt"), "x").unwrap();
        if !o.ancestors().any(|a| a.join(".git").exists()) {
            assert!(locate(&o.join("x.txt")).is_none());
        }
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&o);
    }
}
