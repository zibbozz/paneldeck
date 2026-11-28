//! Git-Operationen über die `git`-CLI: Status, Diff, Stage, Commit und Worktrees.
//! Alle Funktionen blockieren; die UI ruft sie aus Hintergrund-Threads auf.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// Gerät, auf das `core.hooksPath` bei lesenden Befehlen zeigt (dort liegen nie Hooks).
const NO_HOOKS: &str = if cfg!(windows) { "NUL" } else { "/dev/null" };

/// **Der eine** gehärtete Aufbau jedes `git`-Aufrufs der App (#385): Eine fremde `.git/config` (z. B. aus einem entpackten
/// ZIP) darf über automatische Aufrufe (Status, Änderungsmarken, Vergleich) nichts starten und nichts übers Netz holen.
/// - `core.fsmonitor=false` (fsmonitor startet ein Programm), Pfade wörtlich (`--literal-pathspecs`, sonst stagt
///   „a[1].txt“ auch „a1.txt“);
/// - kein Netz: `protocol.allow=never` und je Protokoll (eine Repository-Einstellung `protocol.ext.allow=always` wäre
///   sonst genauer), `GIT_NO_LAZY_FETCH=1` (Partial Clone/Promisor würde sonst beim Lesen eines fehlenden Objekts
///   nachladen – mit `ext::` ein beliebiges Programm), kein Credential-Helfer, keine Rückfragen
///   (`GIT_TERMINAL_PROMPT=0`, `GCM_INTERACTIVE=never`);
/// - `read`: zusätzlich keine Hooks (`core.hooksPath` auf ein Gerät) und keine Signaturprüfung
///   (`log.showSignature=false`, sonst startet `gpg.program`). Schreibende Befehle des Nutzers (Commit, Merge, Worktree)
///   behalten seine Hooks.
pub fn command(dir: &Path, read: bool) -> Command {
    let mut cmd = Command::new("git");
    cmd.args(["-c", "core.fsmonitor=false", "-c", "protocol.allow=never"]);
    for p in ["ext", "file", "http", "https", "ssh", "git", "fd"] {
        cmd.arg("-c").arg(format!("protocol.{p}.allow=never"));
    }
    cmd.args(["-c", "credential.helper=", "-c", "core.askPass="]);
    if read {
        cmd.arg("-c").arg(format!("core.hooksPath={NO_HOOKS}"));
        cmd.args(["-c", "log.showSignature=false", "-c", "core.quotepath=off"]);
    }
    cmd.arg("--literal-pathspecs").current_dir(dir);
    cmd.env("GIT_NO_LAZY_FETCH", "1").env("GIT_TERMINAL_PROMPT", "0").env("GCM_INTERACTIVE", "never").env("GIT_PROTOCOL_FROM_USER", "0").env_remove("GIT_ASKPASS").env_remove("SSH_ASKPASS");
    crate::platform::process::hide_window(&mut cmd);
    cmd
}

/// Prozess mit Zeitgrenze ausführen (`stdin` wird geschrieben): danach wird er beendet und ein Fehler gemeldet – eine
/// hängende Freigabe oder ein blockierter Index lässt so nie eine Anzeige ewig warten.
pub fn output_timeout(mut cmd: Command, stdin: Option<Vec<u8>>, timeout: Duration) -> Result<std::process::Output, String> {
    use std::io::{Read, Write};
    use std::process::Stdio;
    cmd.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("{}: {e}", crate::state::t("git nicht gefunden", "git not found")))?;
    let writer = child.stdin.take().zip(stdin).map(|(mut s, data)| {
        std::thread::spawn(move || {
            let _ = s.write_all(&data);
        })
    });
    let mut out = child.stdout.take().ok_or("stdout")?;
    let mut err = child.stderr.take().ok_or("stderr")?;
    let ro = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = out.read_to_end(&mut b);
        b
    });
    let re = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = err.read_to_end(&mut b);
        b
    });
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(crate::state::t("git antwortet nicht (Zeitgrenze überschritten).", "git does not respond (time limit exceeded).").to_string());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => return Err(e.to_string()),
        }
    };
    if let Some(w) = writer {
        let _ = w.join();
    }
    Ok(std::process::Output { status, stdout: ro.join().unwrap_or_default(), stderr: re.join().unwrap_or_default() })
}

pub fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    run(dir, args, false)
}

fn run(dir: &Path, args: &[&str], read: bool) -> Result<String, String> {
    let mut cmd = command(dir, read);
    cmd.args(args);
    let out = cmd.output().map_err(|e| format!("{}: {e}", crate::state::t("git nicht gefunden", "git not found")))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() { String::from_utf8_lossy(&out.stdout).trim().to_string() } else { err })
    }
}

/// Nur lesende Befehle (`status`, `diff`): Filter-Treiber (`filter.<x>.clean`/`smudge`/`process`) aus der Konfiguration
/// des Repositorys selbst (`.git/config`, eingebundene Dateien, `config.worktree`) abschalten. Eine fremde Konfiguration
/// (z. B. aus einem entpackten ZIP) könnte sonst beim automatischen `git status` oder beim Anzeigen eines Diffs beliebige
/// Befehle starten (#253). Filter aus der System-/Benutzerkonfiguration (z. B. Git LFS) bleiben, sie sind die des Nutzers;
/// ebenso der Git-LFS-Filter eines Repositorys, wenn er genau die Standardbefehle von `git lfs install --local` enthält.
pub fn read_only_overrides(dir: &Path) -> Vec<String> {
    let names = match git(dir, &["config", "--show-scope", "--includes", "-z", "--get-regexp", r"^filter\."]) {
        Ok(o) => local_filter_names(&o, true),
        // Kein Treffer: Exit-Code 1 ohne Meldung.
        Err(e) if e.trim().is_empty() => Vec::new(),
        // Ältere git-Versionen kennen `--show-scope` nicht: dann nur die Datei des Repositorys (mit eingebundenen Dateien).
        Err(_) => git(dir, &["config", "--local", "--includes", "-z", "--get-regexp", r"^filter\."]).map(|o| local_filter_names(&o, false)).unwrap_or_default(),
    };
    let mut v = Vec::new();
    for n in names {
        for k in ["clean", "smudge", "process"] {
            v.push("-c".to_string());
            v.push(format!("filter.{n}.{k}="));
        }
        v.push("-c".to_string());
        v.push(format!("filter.{n}.required=false"));
    }
    v
}

/// Standardbefehle des Git-LFS-Filters (`git lfs install --local` schreibt genau diese).
const LFS_STANDARD: [(&str, &str); 3] = [("clean", "git-lfs clean -- %f"), ("smudge", "git-lfs smudge -- %f"), ("process", "git-lfs filter-process")];

/// Namen der abzuschaltenden Filter-Treiber aus der Ausgabe von `git config [--show-scope] -z --get-regexp ^filter\.`
/// (Einträge `[<bereich>\0]<schlüssel>[\n<wert>]\0`); mit `scoped` nur die aus der Konfiguration des Repositorys
/// (`local`/`worktree`, unbekannte Bereiche ebenso). Ausnahme: `filter.lfs` mit genau den Standardbefehlen.
fn local_filter_names(out: &str, scoped: bool) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    let mut it = out.split('\0');
    loop {
        let scope = if scoped {
            match it.next() {
                Some(s) if !s.is_empty() => s,
                _ => break,
            }
        } else {
            "local"
        };
        let Some(kv) = it.next().filter(|s| !s.is_empty()) else { break };
        if matches!(scope.trim(), "system" | "global" | "command") {
            continue;
        }
        let (key, value) = kv.split_once('\n').map(|(k, v)| (k, Some(v))).unwrap_or((kv, None));
        // filter.<name>.<schlüssel>; der Name darf Punkte enthalten.
        let Some(rest) = key.strip_prefix("filter.") else { continue };
        let Some((name, sub)) = rest.rsplit_once('.') else { continue };
        // Nur Namen, die als `-c`-Schlüssel eindeutig bleiben (kein `=`, keine Steuerzeichen).
        if name.is_empty() || name.contains('=') || name.chars().any(|c| c.is_control()) {
            continue;
        }
        if name == "lfs" {
            let sub = sub.to_ascii_lowercase();
            match LFS_STANDARD.iter().find(|(k, _)| *k == sub) {
                // Ein Befehl mit genau dem Standardwert: unbedenklich.
                Some((_, std)) if value == Some(*std) => continue,
                Some(_) => {}
                // Kein Befehl (z. B. `required`): startet nichts.
                None => continue,
            }
        }
        if !v.iter().any(|x| x == name) {
            v.push(name.to_string());
        }
    }
    v
}

/// Wie [`git`], aber für nur lesende Befehle ohne Filter-Treiber des Repositorys ([`read_only_overrides`]).
pub fn git_read(dir: &Path, args: &[&str]) -> Result<String, String> {
    let ov = read_only_overrides(dir);
    let mut all: Vec<&str> = ov.iter().map(String::as_str).collect();
    all.extend_from_slice(args);
    run(dir, &all, true)
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

/// Untermodule nur über den gespeicherten Commit vergleichen, nie in ihnen selbst `git status` ausführen
/// (`--ignore-submodules=dirty`): ihre eigene Konfiguration (Filter, fsmonitor) wird von [`read_only_overrides`] nicht
/// erfasst und könnte sonst Befehle starten (#253). Ein geänderter Commit eines Untermoduls erscheint weiter.
pub const IGNORE_SUBMODULES: &str = "--ignore-submodules=dirty";

pub fn status(repo: &Path) -> Result<Vec<Change>, String> {
    git_read(repo, &["status", "--porcelain=v1", "-z", "-uall", IGNORE_SUBMODULES]).map(|o| parse_status(&o))
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
        // Kein externes Diff-Programm und keine textconv-Treiber (`diff.external`, `diff.<x>.textconv` einer fremden
        // Konfiguration würden sonst beim Anzeigen starten, #253).
        let args: Vec<&str> = if staged {
            vec!["diff", "--no-color", "--no-ext-diff", "--no-textconv", IGNORE_SUBMODULES, "--cached", "--", &c.path]
        } else {
            vec!["diff", "--no-color", "--no-ext-diff", "--no-textconv", IGNORE_SUBMODULES, "--", &c.path]
        };
        git_read(repo, &args).unwrap_or_else(|e| e)
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

    /// #385: jeder Aufruf ist gehärtet (kein Netz, kein Nachladen, keine Helfer; lesend auch ohne Hooks und Signaturen).
    #[test]
    fn every_git_command_is_hardened() {
        let args = |c: &Command| c.get_args().map(|a| a.to_string_lossy().to_string()).collect::<Vec<_>>();
        let env = |c: &Command, k: &str| c.get_envs().find(|(n, _)| *n == k).and_then(|(_, v)| v.map(|v| v.to_string_lossy().to_string()));
        for read in [false, true] {
            let c = command(Path::new("."), read);
            let a = args(&c);
            for want in ["core.fsmonitor=false", "protocol.allow=never", "protocol.ext.allow=never", "protocol.file.allow=never", "credential.helper=", "--literal-pathspecs"] {
                assert!(a.iter().any(|x| x == want), "{want} fehlt: {a:?}");
            }
            assert_eq!(env(&c, "GIT_NO_LAZY_FETCH").as_deref(), Some("1"));
            assert_eq!(env(&c, "GIT_TERMINAL_PROMPT").as_deref(), Some("0"));
            assert_eq!(env(&c, "GCM_INTERACTIVE").as_deref(), Some("never"));
            assert_eq!(a.iter().any(|x| x.starts_with("core.hooksPath=")), read, "Hooks nur beim Lesen aus");
            assert_eq!(a.iter().any(|x| x == "log.showSignature=false"), read);
        }
        // Zeitgrenze: ein hängender Prozess wird beendet
        let mut slow = if cfg!(windows) { Command::new("ping") } else { Command::new("sleep") };
        if cfg!(windows) {
            slow.args(["-n", "30", "127.0.0.1"]);
        } else {
            slow.arg("30");
        }
        crate::platform::process::hide_window(&mut slow);
        let t = std::time::Instant::now();
        assert!(output_timeout(slow, None, Duration::from_millis(300)).is_err());
        assert!(t.elapsed() < Duration::from_secs(10));
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
    fn foreign_filters_and_diff_drivers_never_run_when_reading() {
        // #253: eine fremde .git/config mit Filter-, textconv- und externem Diff-Treiber; status/diff dürfen nichts starten.
        let d = tmp("foreign");
        init(&d);
        std::fs::write(d.join(".gitattributes"), "*.txt filter=evil diff=evil
").unwrap();
        std::fs::write(d.join("a.txt"), "eins
").unwrap();
        stage_all(&d).unwrap();
        commit(&d, "init").unwrap();
        let marker = |n: &str| d.parent().unwrap().join(format!("gitops-marker-{n}-{}", std::process::id()));
        let unix = |p: &Path| p.to_string_lossy().replace('\\', "/");
        for n in ["clean", "textconv", "external"] {
            let _ = std::fs::remove_file(marker(n));
        }
        git(&d, &["config", "filter.evil.clean", &format!("touch '{}'; cat", unix(&marker("clean")))]).unwrap();
        git(&d, &["config", "filter.evil.process", ""]).unwrap();
        git(&d, &["config", "diff.evil.textconv", &format!("touch '{}'; cat", unix(&marker("textconv")))]).unwrap();
        git(&d, &["config", "diff.external", &format!("touch '{}'", unix(&marker("external")))]).unwrap();
        std::fs::write(d.join("a.txt"), "eins
zwei
").unwrap();
        let ch = status(&d).unwrap();
        let a = ch.iter().find(|c| c.path == "a.txt").expect("a.txt geändert");
        let diff = diff_of(&d, a, false);
        assert!(diff.contains("+zwei"), "{diff}");
        assert!(read_only_overrides(&d).iter().any(|o| o == "filter.evil.clean="));
        for n in ["clean", "textconv", "external"] {
            assert!(!marker(n).exists(), "{n} wurde ausgeführt");
        }
        // Gegenprobe: ein gewöhnliches `git diff` startet die Treiber (der Test prüft also wirklich etwas).
        let _ = git(&d, &["diff", "--", "a.txt"]);
        assert!(marker("external").exists() || marker("clean").exists() || marker("textconv").exists(), "Gegenprobe: Treiber liefen nicht");
        for n in ["clean", "textconv", "external"] {
            let _ = std::fs::remove_file(marker(n));
        }
    }

    /// Review zu #253: Untermodule mit eigener (fremder) Konfiguration – status/diff führen dort nichts aus.
    #[test]
    fn submodule_config_never_runs_when_reading() {
        let root = tmp("sub");
        let (sub, sup) = (root.join("subsrc"), root.join("super"));
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(&sup).unwrap();
        init(&sub);
        std::fs::write(sub.join(".gitattributes"), "*.txt filter=evil\n").unwrap();
        std::fs::write(sub.join("a.txt"), "eins\n").unwrap();
        stage_all(&sub).unwrap();
        commit(&sub, "init").unwrap();
        init(&sup);
        std::fs::write(sup.join("x.txt"), "x\n").unwrap();
        let src = sub.to_string_lossy().replace('\\', "/");
        git(&sup, &["-c", "protocol.file.allow=always", "submodule", "add", "-q", &src, "sub"]).unwrap();
        stage_all(&sup).unwrap();
        commit(&sup, "init").unwrap();
        // Fremde Konfiguration im Untermodul (.git/modules/sub/config)
        let marker = root.join("marker");
        let unix = marker.to_string_lossy().replace('\\', "/");
        git(&sup.join("sub"), &["config", "filter.evil.clean", &format!("touch '{unix}'; cat")]).unwrap();
        // Veraltete Stat-Daten (wie frisch entpackt): git müsste die Datei neu einlesen, also den Filter starten
        let touch = |p: &Path| {
            let f = std::fs::OpenOptions::new().write(true).open(p).unwrap();
            f.set_modified(std::time::SystemTime::now() + Duration::from_secs(120)).unwrap();
        };
        touch(&sup.join("sub").join("a.txt"));
        std::fs::write(sup.join("x.txt"), "x\ny\n").unwrap();
        let ch = status(&sup).unwrap();
        let x = ch.iter().find(|c| c.path == "x.txt").expect("x.txt geändert");
        assert!(diff_of(&sup, x, false).contains("+y"));
        assert!(!marker.exists(), "Filter im Untermodul wurde ausgeführt");
        // Ein geänderter Commit des Untermoduls erscheint weiter
        std::fs::write(sup.join("sub").join("b.txt"), "b\n").unwrap();
        git(&sup.join("sub"), &["-c", "filter.evil.clean=", "add", "b.txt"]).unwrap();
        git(&sup.join("sub"), &["-c", "filter.evil.clean=", "-c", "user.name=Test", "-c", "user.email=test@example.invalid", "commit", "-q", "-m", "b"]).unwrap();
        let _ = std::fs::remove_file(&marker);
        touch(&sup.join("sub").join("a.txt"));
        let ch = status(&sup).unwrap();
        let s = ch.iter().find(|c| c.path == "sub").expect("Untermodul mit neuem Commit");
        assert!(diff_of(&sup, s, false).contains("Subproject commit"));
        assert!(!marker.exists(), "Filter im Untermodul wurde ausgeführt");
        // Gegenprobe: ein gewöhnliches `git status` geht ins Untermodul und startet den Filter
        let _ = git(&sup, &["status", "--porcelain"]);
        assert!(marker.exists(), "Gegenprobe: Filter lief nicht");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn filter_names_only_from_the_repository() {
        let out = "local\0filter.evil.clean\ntouch x\0global\0filter.lfs.process\ngit-lfs filter-process\0worktree\0filter.a.b.smudge\nx\0system\0filter.s.clean\nx\0command\0filter.c.clean\nx\0local\0filter.evil.smudge\0local\0filter.a b.clean\n\0";
        assert_eq!(local_filter_names(out, true), vec!["evil", "a.b", "a b"]);
        assert_eq!(local_filter_names("filter.x.clean\ny\0filter.x.process\nz\0", false), vec!["x"]);
        assert!(local_filter_names("local\0filter.bad=x.clean\ny\0", true).is_empty());
        assert!(local_filter_names("", true).is_empty());
    }

    /// Review zu #253: `git lfs install --local` – der Standard-LFS-Filter des Repositorys bleibt, alles andere nicht.
    #[test]
    fn standard_lfs_filter_of_the_repository_stays() {
        let std_lfs = "local\0filter.lfs.clean\ngit-lfs clean -- %f\0local\0filter.lfs.smudge\ngit-lfs smudge -- %f\0local\0filter.lfs.process\ngit-lfs filter-process\0local\0filter.lfs.required\ntrue\0";
        assert!(local_filter_names(std_lfs, true).is_empty());
        assert!(local_filter_names(&std_lfs.replace("local\0", ""), false).is_empty());
        // Ein abweichender Befehl (auch leer, ohne Wert oder anders geschrieben) schaltet den ganzen Filter ab
        for bad in ["filter.lfs.process\ngit-lfs filter-process; touch x", "filter.lfs.clean\n", "filter.lfs.smudge", "filter.lfs.clean\ngit-lfs  clean -- %f", "filter.LFS.clean\ngit-lfs clean -- %f; x"] {
            let out = format!("{std_lfs}local\0{bad}\0");
            assert!(!local_filter_names(&out, true).is_empty(), "{bad:?}");
        }
        // Echtes Repository: nur der Standard-LFS-Filter → keine Überschreibung
        let d = tmp("lfs");
        init(&d);
        for (k, v) in LFS_STANDARD {
            git(&d, &["config", &format!("filter.lfs.{k}"), v]).unwrap();
        }
        git(&d, &["config", "filter.lfs.required", "true"]).unwrap();
        assert!(!read_only_overrides(&d).iter().any(|o| o.starts_with("filter.lfs.")), "{:?}", read_only_overrides(&d));
        git(&d, &["config", "filter.lfs.process", "sh -c 'touch x'"]).unwrap();
        assert!(read_only_overrides(&d).iter().any(|o| o == "filter.lfs.process="));
        let _ = std::fs::remove_dir_all(&d);
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
