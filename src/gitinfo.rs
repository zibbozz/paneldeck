//! Git-Branch und Anzahl geänderter Dateien pro Sitzungsordner (Hintergrund-Thread).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub struct GitInfo {
    pub branch: String,
    pub changed: usize,
}

fn find_repo(start: &Path) -> Option<PathBuf> {
    let mut p = Some(start);
    while let Some(d) = p {
        if d.join(".git").exists() {
            return Some(d.to_path_buf());
        }
        p = d.parent();
    }
    None
}

fn branch_of(repo: &Path) -> Option<String> {
    let git = repo.join(".git");
    let git = if git.is_file() {
        // Worktree/Submodule: "gitdir: <pfad>"
        let s = std::fs::read_to_string(&git).ok()?;
        let rel = s.trim().strip_prefix("gitdir:")?.trim().to_string();
        let p = PathBuf::from(&rel);
        if p.is_absolute() { p } else { repo.join(p) }
    } else {
        git
    };
    let head = std::fs::read_to_string(git.join("HEAD")).ok()?;
    let head = head.trim();
    Some(match head.strip_prefix("ref: refs/heads/") {
        Some(b) => b.to_string(),
        None => format!("{}…", head.chars().take(7).collect::<String>()),
    })
}

fn changed_files(repo: &Path) -> usize {
    // Gehärteter Aufruf (#385), keine Filter-Treiber aus der Konfiguration des Repositorys und kein `git status` in
    // Untermodulen (#253): der Aufruf läuft automatisch alle paar Sekunden.
    let mut cmd = crate::gitops::command(repo, true);
    cmd.args(crate::gitops::read_only_overrides(repo)).args(["--no-optional-locks", "status", "--porcelain", crate::gitops::IGNORE_SUBMODULES]);
    cmd.output().map(|o| String::from_utf8_lossy(&o.stdout).lines().count()).unwrap_or(0)
}

pub fn query(dir: &Path) -> Option<GitInfo> {
    let repo = find_repo(dir)?;
    Some(GitInfo { branch: branch_of(&repo)?, changed: changed_files(&repo) })
}

#[derive(Clone, Default)]
pub struct GitWatcher {
    dirs: Arc<Mutex<HashMap<u64, PathBuf>>>,
    infos: Arc<Mutex<HashMap<u64, Option<GitInfo>>>>,
}

impl GitWatcher {
    pub fn start(ctx: eframe::egui::Context) -> Self {
        let w = GitWatcher::default();
        let (dirs, infos) = (w.dirs.clone(), w.infos.clone());
        std::thread::spawn(move || loop {
            let snapshot: Vec<(u64, PathBuf)> = dirs.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(k, v)| (*k, v.clone())).collect();
            let mut changed = false;
            let mut cache: HashMap<PathBuf, Option<GitInfo>> = HashMap::new();
            for (id, dir) in snapshot {
                let info = cache.entry(dir.clone()).or_insert_with(|| query(&dir)).clone();
                let mut m = infos.lock().unwrap_or_else(|e| e.into_inner());
                if m.get(&id) != Some(&info) {
                    m.insert(id, info);
                    changed = true;
                }
            }
            if changed {
                ctx.request_repaint();
            }
            std::thread::sleep(Duration::from_secs(4));
        });
        w
    }

    pub fn set_dir(&self, id: u64, dir: PathBuf) {
        self.dirs.lock().unwrap_or_else(|e| e.into_inner()).insert(id, dir);
    }

    pub fn get(&self, id: u64) -> Option<GitInfo> {
        self.infos.lock().unwrap_or_else(|e| e.into_inner()).get(&id).cloned().flatten()
    }

    pub fn remove(&self, id: u64) {
        self.dirs.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        self.infos.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_branch_and_changes() {
        let dir = std::env::temp_dir().join(format!("git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| Command::new("git").args(args).current_dir(&dir).output().unwrap();
        run(&["init", "-q", "-b", "feature-x"]);
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        let sub = dir.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let info = query(&sub).unwrap(); // aus Unterordner gefunden
        assert_eq!(info.branch, "feature-x");
        assert_eq!(info.changed, 1);
    }
}
