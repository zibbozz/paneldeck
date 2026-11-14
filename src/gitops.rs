//! Git-Operationen über die `git`-CLI: Status, Diff, Stage, Commit und Worktrees.
//! Alle Funktionen blockieren; die UI ruft sie aus Hintergrund-Threads auf.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

pub fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    // core.fsmonitor aus: eine fremde .git/config (z. B. aus einem entpackten ZIP) könnte darüber beliebige
    // Befehle starten, sobald die Hub-Ansicht automatisch `git status` aufruft. Pfade immer wörtlich
    // nehmen (sonst stagt „a[1].txt“ auch „a1.txt“).
    cmd.args(["-c", "core.fsmonitor=false", "--literal-pathspecs"]).args(args).current_dir(dir);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().map_err(|e| format!("{}: {e}", crate::state::t("git nicht gefunden", "git not found")))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() { String::from_utf8_lossy(&out.stdout).trim().to_string() } else { err })
    }
}

pub fn repo_root(dir: &Path) -> Option<PathBuf> {
    git(dir, &["rev-parse", "--show-toplevel"]).ok().map(|s| PathBuf::from(s.trim()))
}

/// Schnelle Prüfung ohne Prozessstart (für Eingabefelder): liegt `dir` in einem Git-Repository?
pub fn inside_repo_fs(dir: &Path) -> bool {
    dir.is_dir() && dir.ancestors().any(|d| d.join(".git").exists())
}

#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub path: String,
    /// Status im Index (X) und im Arbeitsverzeichnis (Y) wie bei `git status --porcelain`.
    pub x: char,
    pub y: char,
}

impl Change {
    pub fn staged(&self) -> bool {
        self.x != ' ' && self.x != '?'
    }
    pub fn unstaged(&self) -> bool {
        self.y != ' '
    }
    pub fn untracked(&self) -> bool {
        self.x == '?'
    }
}

pub fn parse_status(out: &str) -> Vec<Change> {
    let mut v = Vec::new();
    let mut it = out.split('\0').filter(|s| !s.is_empty());
    while let Some(e) = it.next() {
        let b: Vec<char> = e.chars().collect();
        if b.len() < 4 {
            continue;
        }
        let (x, y) = (b[0], b[1]);
        let path: String = b[3..].iter().collect();
        if x == 'R' || x == 'C' || y == 'R' || y == 'C' {
            let _ = it.next(); // ursprünglicher Pfad
        }
        v.push(Change { path, x, y });
    }
    v
}

pub fn status(repo: &Path) -> Result<Vec<Change>, String> {
    git(repo, &["status", "--porcelain=v1", "-z", "-uall"]).map(|o| parse_status(&o))
}

/// Diff einer Datei: `staged` = Index gegen HEAD, sonst Arbeitsverzeichnis gegen Index.
pub fn diff_of(repo: &Path, c: &Change, staged: bool) -> String {
    const MAX: usize = 400_000;
    let text = if c.untracked() {
        // Nur den Anfang lesen – eine neue, riesige Datei soll nicht komplett in den Speicher.
        let read_head = |p: &Path| -> std::io::Result<String> {
            use std::io::Read;
            let mut b = Vec::new();
            std::fs::File::open(p)?.take(MAX as u64).read_to_end(&mut b)?;
            match String::from_utf8(b) {
                Ok(t) => Ok(t),
                // Am Ende abgeschnittenes Mehrbyte-Zeichen ist kein Fehler; echte Binärdaten schon.
                Err(e) if e.utf8_error().error_len().is_none() => {
                    let n = e.utf8_error().valid_up_to();
                    let mut b = e.into_bytes();
                    b.truncate(n);
                    Ok(String::from_utf8(b).unwrap_or_default())
                }
                Err(_) => Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "binary")),
            }
        };
        match read_head(&repo.join(&c.path)) {
            Ok(t) => {
                let body: String = t.lines().take(2000).map(|l| format!("+{l}\n")).collect();
                format!("{}: {}\n@@ -0,0 +1,{} @@\n{body}", crate::state::t("neue Datei", "new file"), c.path, t.lines().count())
            }
            Err(_) => format!("{}: {} {}", crate::state::t("neue Datei", "new file"), c.path, crate::state::t("(nicht als Text lesbar)", "(not readable as text)")),
        }
    } else {
        let args: Vec<&str> = if staged {
            vec!["diff", "--no-color", "--cached", "--", &c.path]
        } else {
            vec!["diff", "--no-color", "--", &c.path]
        };
        git(repo, &args).unwrap_or_else(|e| e)
    };
    if text.len() > MAX {
        let mut t = text;
        // An einer Zeichengrenze kürzen (truncate mitten in einem Umlaut/Emoji würde paniken → Absturz).
        let mut cut = MAX;
        while !t.is_char_boundary(cut) {
            cut -= 1;
        }
        t.truncate(cut);
        t.push_str(crate::state::t("\n… (Diff gekürzt)", "\n… (diff truncated)"));
        t
    } else {
        text
    }
}

pub fn stage(repo: &Path, path: &str) -> Result<(), String> {
    git(repo, &["add", "--", path]).map(|_| ())
}

pub fn unstage(repo: &Path, path: &str) -> Result<(), String> {
    git(repo, &["restore", "--staged", "--", path])
        .or_else(|_| git(repo, &["reset", "-q", "HEAD", "--", path]))
        .map(|_| ())
}

pub fn stage_all(repo: &Path) -> Result<(), String> {
    git(repo, &["add", "-A"]).map(|_| ())
}

pub fn commit(repo: &Path, msg: &str) -> Result<String, String> {
    git(repo, &["commit", "-m", msg])
}

pub fn current_branch(repo: &Path) -> String {
    git(repo, &["rev-parse", "--abbrev-ref", "HEAD"]).map(|s| s.trim().to_string()).unwrap_or_default()
}

// ------------------------------------------------------------ Worktrees

/// Zielordner für einen Worktree: `<repo>-worktrees/<branch mit / → ->`.
pub fn worktree_path(repo: &Path, branch: &str) -> PathBuf {
    let name = repo.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "repo".into());
    let parent = repo.parent().unwrap_or(repo);
    parent.join(format!("{name}-worktrees")).join(branch.replace(['/', '\\', ':'], "-"))
}

pub fn worktree_add(repo: &Path, branch: &str, path: &Path) -> Result<(), String> {
    if let Some(p) = path.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    git(repo, &["worktree", "add", "-b", branch, &path.to_string_lossy()]).map(|_| ())
}

pub fn worktree_remove(repo: &Path, path: &Path, force: bool) -> Result<(), String> {
    let p = path.to_string_lossy().to_string();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&p);
    git(repo, &args).map(|_| ())
}

pub fn branch_delete(repo: &Path, branch: &str, force: bool) -> Result<(), String> {
    git(repo, &["branch", if force { "-D" } else { "-d" }, branch]).map(|_| ())
}

/// Branch in den aktuell ausgecheckten Branch des Haupt-Repos mergen; bei Konflikt wird abgebrochen.
pub fn merge(repo: &Path, branch: &str) -> Result<(), String> {
    // Läuft im Haupt-Repo schon ein Merge des Nutzers, nicht dazwischenfunken – ein „merge --abort“
    // würde sonst dessen halb gelöste Konflikte verwerfen.
    if git(repo, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).is_ok() {
        return Err(crate::state::t("im Haupt-Repository läuft bereits ein Merge", "a merge is already in progress in the main repository").into());
    }
    match git(repo, &["merge", "--no-ff", "--no-edit", branch]) {
        Ok(_) => Ok(()),
        Err(e) => {
            let _ = git(repo, &["merge", "--abort"]);
            Err(e)
        }
    }
}

// ------------------------------------------------------------ Hintergrund-Beobachter für das Änderungsfenster

#[derive(Clone, Default)]
pub struct Snapshot {
    pub repo: Option<PathBuf>,
    pub branch: String,
    pub changes: Vec<Change>,
    pub diff: String,
    pub error: Option<String>,
}

#[derive(Clone, Default)]
struct Target {
    dir: Option<PathBuf>,
    selected: Option<(Change, bool)>,
}

#[derive(Default)]
struct Inner {
    target: Target,
    snap: Snapshot,
    kick: bool,
    stop: bool,
}

#[derive(Clone)]
pub struct DiffWatcher {
    inner: Arc<(Mutex<Inner>, Condvar)>,
}

impl DiffWatcher {
    pub fn start(ctx: eframe::egui::Context) -> Self {
        let inner = Arc::new((Mutex::new(Inner::default()), Condvar::new()));
        let w = DiffWatcher { inner: inner.clone() };
        std::thread::spawn(move || loop {
            let target = {
                let (m, cv) = &*inner;
                let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
                if !g.kick {
                    g = cv.wait_timeout(g, Duration::from_millis(1500)).unwrap_or_else(|e| e.into_inner()).0;
                }
                g.kick = false;
                if g.stop {
                    return;
                }
                g.target.clone()
            };
            let snap = match &target.dir {
                None => Snapshot::default(),
                Some(dir) => match repo_root(dir) {
                    None => Snapshot { error: Some(crate::state::t("Kein Git-Repository", "Not a Git repository").into()), ..Snapshot::default() },
                    Some(repo) => match status(&repo) {
                        Err(e) => Snapshot { repo: Some(repo), error: Some(e), ..Snapshot::default() },
                        Ok(changes) => {
                            let diff = target
                                .selected
                                .as_ref()
                                .and_then(|(c, staged)| changes.iter().find(|x| x.path == c.path).map(|x| diff_of(&repo, x, *staged)))
                                .unwrap_or_default();
                            Snapshot { branch: current_branch(&repo), repo: Some(repo), changes, diff, error: None }
                        }
                    },
                },
            };
            let (m, _) = &*inner;
            let changed = {
                let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
                let changed = g.snap.changes != snap.changes || g.snap.diff != snap.diff || g.snap.error != snap.error || g.snap.branch != snap.branch;
                g.snap = snap;
                changed
            };
            if changed {
                ctx.request_repaint();
            }
        });
        w
    }

    /// Ordner und ausgewählte Datei setzen (`None` = nicht beobachten).
    pub fn set_target(&self, dir: Option<PathBuf>, selected: Option<(Change, bool)>) {
        let (m, cv) = &*self.inner;
        let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
        let new = Target { dir, selected };
        let changed = g.target.dir != new.dir || g.target.selected != new.selected;
        g.target = new;
        if changed {
            g.kick = true;
            cv.notify_all();
        }
    }

    pub fn refresh(&self) {
        let (m, cv) = &*self.inner;
        m.lock().unwrap_or_else(|e| e.into_inner()).kick = true;
        cv.notify_all();
    }

    pub fn snapshot(&self) -> Snapshot {
        self.inner.0.lock().unwrap_or_else(|e| e.into_inner()).snap.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gitops-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn init(d: &Path) {
        git(d, &["init", "-q", "-b", "main"]).unwrap();
        git(d, &["config", "user.email", "t@example.com"]).unwrap();
        git(d, &["config", "user.name", "Test"]).unwrap();
    }

    #[test]
    fn parse_porcelain() {
        let c = parse_status("M  a.txt\0 M b.txt\0?? c.txt\0R  neu.txt\0alt.txt\0");
        assert_eq!(c.len(), 4);
        assert!(c[0].staged() && !c[0].unstaged());
        assert!(!c[1].staged() && c[1].unstaged());
        assert!(c[2].untracked());
        assert_eq!(c[3].path, "neu.txt");
    }

    #[test]
    fn stage_commit_flow() {
        let d = tmp("flow");
        init(&d);
        std::fs::write(d.join("a.txt"), "eins\n").unwrap();
        stage_all(&d).unwrap();
        commit(&d, "init").unwrap();
        std::fs::write(d.join("a.txt"), "eins\nzwei\n").unwrap();
        std::fs::write(d.join("neu.txt"), "x\n").unwrap();
        let ch = status(&d).unwrap();
        assert_eq!(ch.len(), 2);
        let a = ch.iter().find(|c| c.path == "a.txt").unwrap();
        assert!(diff_of(&d, a, false).contains("+zwei"));
        stage(&d, "a.txt").unwrap();
        let ch = status(&d).unwrap();
        assert!(ch.iter().find(|c| c.path == "a.txt").unwrap().staged());
        assert!(diff_of(&d, ch.iter().find(|c| c.path == "a.txt").unwrap(), true).contains("+zwei"));
        unstage(&d, "a.txt").unwrap();
        assert!(!status(&d).unwrap().iter().find(|c| c.path == "a.txt").unwrap().staged());
        let n = status(&d).unwrap().into_iter().find(|c| c.path == "neu.txt").unwrap();
        assert!(diff_of(&d, &n, false).contains("+x"));
    }

    #[test]
    fn worktree_lifecycle() {
        let d = tmp("wt");
        init(&d);
        std::fs::write(d.join("a.txt"), "1\n").unwrap();
        stage_all(&d).unwrap();
        commit(&d, "init").unwrap();
        let wt = worktree_path(&d, "claude/test");
        worktree_add(&d, "claude/test", &wt).unwrap();
        assert!(wt.join("a.txt").exists());
        std::fs::write(wt.join("b.txt"), "neu\n").unwrap();
        stage_all(&wt).unwrap();
        commit(&wt, "in worktree").unwrap();
        merge(&d, "claude/test").unwrap();
        assert!(d.join("b.txt").exists());
        worktree_remove(&d, &wt, false).unwrap();
        branch_delete(&d, "claude/test", false).unwrap();
        assert!(!wt.exists());
    }
}
