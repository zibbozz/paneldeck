//! Git states of a file for the comparison and the change markers (#385, 3.2.0) – **only the local git CLI**, nothing
//! goes over the network. All functions block (git runs as a process); the editor calls them from helper threads.
//!
//! Safety as in `gitops`: `core.fsmonitor=false` (a foreign `.git/config` could otherwise start programs),
//! literal pathspecs, hidden window. Contents come from `git cat-file --batch` without `--filters`/`--textconv`, so no
//! filter or textconv driver of the repository runs; refs typed by the user are checked ([`check_ref`]) and verified
//! (`rev-parse --verify`) before use.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A file inside a git repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoFile {
    /// Root of the working tree.
    pub root: PathBuf,
    /// The repository folder (`.git`, or the folder of a worktree).
    pub git_dir: PathBuf,
    /// Path relative to the root, with `/`.
    pub rel: String,
}

/// Which state of the file.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Rev {
    Head,
    /// The index (staged state).
    Index,
    /// A commit, branch or tag (checked with [`check_ref`]).
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

    /// Short label (`HEAD`, `index`, the ref).
    pub fn label(&self) -> String {
        match self {
            Rev::Head => "HEAD".into(),
            Rev::Index => "index".into(),
            Rev::Commit(r) => r.clone(),
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

/// Content of a state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Blob {
    Bytes(Vec<u8>),
    /// Not in this state (new file, not in the index, unknown commit).
    Missing,
    /// Larger than allowed (size in bytes).
    TooLarge(u64),
}

fn cmd(dir: &Path) -> Command {
    let mut c = Command::new("git");
    c.args(["-c", "core.fsmonitor=false", "-c", "core.quotepath=off", "--literal-pathspecs"]).current_dir(dir);
    crate::platform::process::hide_window(&mut c);
    c
}

fn run(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = cmd(dir).args(args).stdin(Stdio::null()).output().map_err(|e| format!("{}: {e}", crate::state::t("git nicht gefunden", "git not found")))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Is git there at all? (Tests überspringen ohne git.)
#[cfg(test)]
pub fn available() -> bool {
    let mut c = Command::new("git");
    c.arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).stdin(Stdio::null());
    crate::platform::process::hide_window(&mut c);
    c.status().is_ok_and(|s| s.success())
}

/// Repository of a file (`None`: not in a repository, or git missing).
pub fn locate(path: &Path) -> Option<RepoFile> {
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
    // Innerhalb von `.git` selbst gibt es kein Arbeitsverzeichnis.
    Some(RepoFile { root, git_dir, rel: format!("{prefix}{name}") })
}

/// Contents of several states in one `git cat-file --batch` (each at most `max` bytes).
pub fn blobs(repo: &RepoFile, revs: &[Rev], max: u64) -> Result<Vec<Blob>, String> {
    let mut child = cmd(&repo.root)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{}: {e}", crate::state::t("git nicht gefunden", "git not found")))?;
    let mut input = String::new();
    for r in revs {
        // Zeilenumbrüche im Pfad würden die Anfrage teilen: so ein Pfad hat hier keinen Stand.
        if repo.rel.contains(['\n', '\r']) {
            input.push_str("\0invalid\n");
        } else {
            input.push_str(&r.spec(&repo.rel));
            input.push('\n');
        }
    }
    let mut stdin = child.stdin.take().ok_or("stdin")?;
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(input.as_bytes());
    });
    let mut out = Vec::new();
    child.stdout.take().ok_or("stdout")?.read_to_end(&mut out).map_err(|e| e.to_string())?;
    let _ = writer.join();
    let _ = child.wait();
    parse_batch(&out, revs.len(), max)
}

/// Output of `cat-file --batch`: per object `<sha> <type> <size>\n<content>\n` or `<name> missing\n`.
fn parse_batch(out: &[u8], n: usize, max: u64) -> Result<Vec<Blob>, String> {
    let mut v = Vec::with_capacity(n);
    let mut pos = 0usize;
    while v.len() < n {
        let Some(nl) = out[pos..].iter().position(|b| *b == b'\n') else { break };
        let head = String::from_utf8_lossy(&out[pos..pos + nl]).to_string();
        pos += nl + 1;
        // `<name> missing` / `<name> ambiguous` (der Name kann Leerzeichen enthalten), sonst `<sha> <typ> <größe>`.
        let mut parts: Vec<&str> = head.rsplitn(3, ' ').collect();
        parts.reverse();
        if parts.len() == 3 && !head.ends_with(" missing") && !head.ends_with(" ambiguous") && parts[2].bytes().all(|b| b.is_ascii_digit()) {
            let size: usize = parts[2].trim().parse().map_err(|_| format!("cat-file: {head}"))?;
            let end = (pos + size).min(out.len());
            let body = &out[pos..end];
            pos = (end + 1).min(out.len());
            v.push(if parts[1] != "blob" {
                Blob::Missing
            } else if size as u64 > max {
                Blob::TooLarge(size as u64)
            } else {
                Blob::Bytes(body.to_vec())
            });
        } else {
            v.push(Blob::Missing);
        }
    }
    while v.len() < n {
        v.push(Blob::Missing);
    }
    Ok(v)
}

/// Is `r` a plausible ref the user typed (branch, tag, commit, `HEAD~2`, `main^`)? No options, no ranges, no spaces.
pub fn check_ref(r: &str) -> Result<(), String> {
    let ok = !r.is_empty() && r.len() <= 100 && !r.starts_with('-') && !r.contains("..") && !r.ends_with('.') && !r.ends_with(".lock") && r.chars().all(|c| c.is_ascii_alphanumeric() || "_./~^@-".contains(c));
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

/// Branches and tags (short names), at most 300 each.
pub fn refs(root: &Path) -> (Vec<String>, Vec<String>) {
    let out = run(root, &["for-each-ref", "--format=%(refname)", "--count=600", "refs/heads", "refs/tags"]).unwrap_or_default();
    let mut branches = Vec::new();
    let mut tags = Vec::new();
    for l in out.lines() {
        if let Some(b) = l.strip_prefix("refs/heads/") {
            if branches.len() < 300 {
                branches.push(b.to_string());
            }
        } else if let Some(t) = l.strip_prefix("refs/tags/")
            && tags.len() < 300
        {
            tags.push(t.to_string());
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

/// The last `n` commits that changed the file.
pub fn file_log(repo: &RepoFile, n: usize) -> Vec<Commit> {
    let count = format!("-n{}", n.clamp(1, 500));
    let out = run(&repo.root, &["log", &count, "--no-color", "--date=short", "--format=%H%x1f%h%x1f%ad%x1f%an%x1f%s", "--", &repo.rel]).unwrap_or_default();
    out.lines()
        .filter_map(|l| {
            let p: Vec<&str> = l.split('\u{1f}').collect();
            (p.len() == 5).then(|| Commit { hash: p[0].into(), short: p[1].into(), date: p[2].into(), author: p[3].into(), subject: p[4].chars().take(200).collect() })
        })
        .collect()
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
        let st = cmd(dir).args(["-c", "user.name=Test", "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false"]).args(args).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap();
        assert!(st.success(), "git {args:?}");
    }

    #[test]
    fn refs_are_checked_before_use() {
        for ok in ["main", "feature/x-1", "v1.2.3", "HEAD~2", "main^", "a1b2c3d", "@"] {
            assert!(check_ref(ok).is_ok(), "{ok}");
        }
        for bad in ["", "-n", "--all", "a..b", "a b", "x;rm", "a\nb", "ref.lock", "x.", &"a".repeat(101), "$(x)", "a:b"] {
            assert!(check_ref(bad).is_err(), "{bad:?}");
        }
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
        assert_eq!((br, tags), (vec!["main".to_string()], vec!["v1".to_string()]));
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
