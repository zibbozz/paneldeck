//! Datenordner: einmal pro Prozess auflösen und zwischenspeichern.
//!
//! Reihenfolge: `--data-dir` > Umgebungsvariable `PANELDECK_DATA` > portabler Ordner `paneldeck-data` neben der .exe
//! (wenn beschreibbar) > `%APPDATA%\Paneldeck` > `.` (Notlösung). Linux/macOS (#149/#150): statt `%APPDATA%` der
//! Benutzerordner des Systems (`$XDG_DATA_HOME` bzw. `~/.local/share`, macOS `~/Library/Application Support`); der
//! portable Ordner zählt dort nur, wenn es ihn schon gibt (eine AppImage: neben der `.AppImage`-Datei), damit ein
//! entpacktes Archiv nicht stillschweigend zum Datenordner wird. `--profile <name>` hängt `profiles\<name>` an den
//! gefundenen Ordner an. Ein ausdrücklich angegebener Ordner (Argument, Umgebungsvariable, Profil), der sich nicht
//! anlegen oder beschreiben lässt, ist ein Fehler – kein stiller Rückfall auf einen anderen Ordner.

use crate::cli::Cli;
use crate::state::Lang;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Umgebungsvariable für einen festen Datenordner (öffentlich, siehe `--help`).
pub const ENV_DATA: &str = "PANELDECK_DATA";
/// Datenordner des Hubs, der ein Terminal gestartet hat (intern; setzt der Hub in jedes Terminal). Damit findet
/// `paneldeck.exe mcp` aus einem Paneldeck-Terminal heraus genau diesen Hub, auch bei `--data-dir`/`--profile`.
pub const ENV_HUB_DATA: &str = "PANELDECK_HUB_DATA";
/// Basisordner (ohne Profil) dieses Hubs (intern): `paneldeck.exe mcp --profile p` aus einem Hub-Terminal löst das
/// Profil unter derselben Basis auf wie der Hub (auch wenn der Hub mit `--data-dir` gestartet wurde).
pub const ENV_HUB_BASE: &str = "PANELDECK_HUB_BASE";

/// Woher der Datenordner stammt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Arg,
    Env,
    Portable,
    AppData,
    Fallback,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataDir {
    /// Tatsächlich benutzter Ordner (mit Profil: `<base>\profiles\<name>`).
    pub path: PathBuf,
    /// Ordner ohne Profil.
    pub base: PathBuf,
    pub source: Source,
    pub profile: Option<String>,
}

/// Eingaben der Auflösung (in Tests frei wählbar, im Programm aus Kommandozeile und Umgebung).
#[derive(Clone, Debug, Default)]
pub struct Inputs {
    /// `--data-dir`, absolut.
    pub arg: Option<PathBuf>,
    /// `PANELDECK_DATA`, absolut (leer = nicht gesetzt).
    pub env: Option<PathBuf>,
    /// Ordner der .exe.
    pub exe_dir: Option<PathBuf>,
    /// `%APPDATA%` (sonst `HOME`).
    pub appdata: Option<PathBuf>,
    pub profile: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathError {
    pub path: PathBuf,
    pub source: Source,
}

impl PathError {
    pub fn message(&self, l: Lang) -> String {
        let why = match self.source {
            Source::Arg => l.t("angegeben mit --data-dir", "given with --data-dir"),
            Source::Env => l.t("angegeben mit PANELDECK_DATA", "given with PANELDECK_DATA"),
            _ => l.t("Profilordner", "profile folder"),
        };
        format!(
            "Paneldeck: {} {} ({why}).",
            l.t("Der Datenordner lässt sich nicht anlegen oder beschreiben:", "The data folder cannot be created or written:"),
            self.path.display()
        )
    }
}

/// Ordner anlegen und Schreibprobe machen.
pub fn usable(p: &Path) -> bool {
    if std::fs::create_dir_all(p).is_err() {
        return false;
    }
    let probe = p.join(".probe");
    if std::fs::write(&probe, b"x").is_err() {
        return false;
    }
    let _ = std::fs::remove_file(probe);
    true
}

/// Reine Auflösung (ohne globale Zustände); `usable` prüft bzw. legt einen Ordner an.
pub fn resolve_with(inp: &Inputs, usable: &dyn Fn(&Path) -> bool) -> Result<DataDir, PathError> {
    let (base, source) = if let Some(a) = &inp.arg {
        if !usable(a) {
            return Err(PathError { path: a.clone(), source: Source::Arg });
        }
        (a.clone(), Source::Arg)
    } else if let Some(e) = &inp.env {
        if !usable(e) {
            return Err(PathError { path: e.clone(), source: Source::Env });
        }
        (e.clone(), Source::Env)
    } else if let Some(p) = inp.exe_dir.as_ref().map(|d| d.join("paneldeck-data")).filter(|p| (cfg!(windows) || p.is_dir()) && usable(p)) {
        (p, Source::Portable)
    } else if let Some(p) = inp.appdata.as_ref().map(|d| d.join("Paneldeck")).filter(|p| usable(p)) {
        (p, Source::AppData)
    } else {
        (std::path::absolute(".").unwrap_or_else(|_| PathBuf::from(".")), Source::Fallback)
    };
    let path = match &inp.profile {
        Some(name) => {
            let p = base.join("profiles").join(name);
            if !usable(&p) {
                return Err(PathError { path: p, source });
            }
            p
        }
        None => base.clone(),
    };
    Ok(DataDir { path, base, source, profile: inp.profile.clone() })
}

fn env_path(name: &str) -> Option<PathBuf> {
    let v = std::env::var_os(name)?;
    if v.to_string_lossy().trim().is_empty() {
        return None;
    }
    Some(crate::cli::abs_from_cwd(&v.to_string_lossy()))
}

/// Eingaben aus Kommandozeile und echter Umgebung.
pub fn inputs(cli: &Cli) -> Inputs {
    Inputs {
        arg: cli.data_dir.clone(),
        env: env_path(ENV_DATA),
        exe_dir: portable_base(),
        appdata: user_data_root(),
        profile: cli.profile.clone(),
    }
}

/// Ordner, neben dem der portable Ordner `paneldeck-data` liegt: der Ordner der Programmdatei; bei einer AppImage
/// (`$APPIMAGE`, die Programmdatei liegt dann in einem schreibgeschützten Einhängepunkt) der Ordner der `.AppImage`.
fn portable_base() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    if let Some(a) = std::env::var_os("APPIMAGE").filter(|a| !a.is_empty()) {
        return PathBuf::from(a).parent().map(Path::to_path_buf);
    }
    std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.to_path_buf()))
}

/// Benutzerordner für Programmdaten, unter dem `Paneldeck` liegt: Windows `%APPDATA%`, Linux `$XDG_DATA_HOME`
/// (absolut) bzw. `~/.local/share`, macOS `~/Library/Application Support`; Notlösung `HOME`.
pub fn user_data_root() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from);
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA").map(PathBuf::from).or(home)
    }
    #[cfg(target_os = "macos")]
    {
        home.map(|h| h.join("Library").join("Application Support"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        xdg_data_home(std::env::var_os("XDG_DATA_HOME").map(PathBuf::from), home)
    }
}

/// `$XDG_DATA_HOME`, wenn absolut, sonst `$HOME/.local/share` (XDG Base Directory Specification).
#[cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]
pub fn xdg_data_home(xdg: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    xdg.filter(|p| p.is_absolute()).or_else(|| home.map(|h| h.join(".local").join("share")))
}

/// Linux/macOS (#149 Review): Datenordner nur für den Benutzer (0700), auch einen schon vorhandenen. Der Basisordner
/// eines Profils ebenso, außer bei `--data-dir`/`PANELDECK_DATA` (fremder Ordner des Nutzers: nur der Profilordner).
fn harden(d: &DataDir) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut dirs = vec![d.path.clone()];
        if d.base != d.path && !matches!(d.source, Source::Arg | Source::Env) {
            dirs.push(d.base.clone());
        }
        for p in dirs {
            if std::fs::metadata(&p).is_ok_and(|m| m.is_dir() && m.permissions().mode() & 0o077 != 0) {
                let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700));
            }
        }
    }
    #[cfg(not(unix))]
    let _ = d;
}

static DIR: OnceLock<DataDir> = OnceLock::new();

/// Datenordner für diesen Prozess auflösen (einmal; spätere Aufrufe liefern das erste Ergebnis).
pub fn init(cli: &Cli) -> Result<&'static DataDir, PathError> {
    if let Some(d) = DIR.get() {
        return Ok(d);
    }
    let d = resolve_with(&inputs(cli), &usable)?;
    harden(&d);
    Ok(DIR.get_or_init(|| d))
}

/// Aufgelöster Datenordner. Ohne vorheriges `init` wird mit der Kommandozeile des Prozesses aufgelöst;
/// schlägt das fehl (nur möglich, wenn `main` den Fehler nicht abgefangen hat), gilt die Notlösung `.`.
pub fn get() -> &'static DataDir {
    if let Some(d) = DIR.get() {
        return d;
    }
    match init(crate::cli::get()) {
        Ok(d) => d,
        Err(_) => {
            let dot = std::path::absolute(".").unwrap_or_else(|_| PathBuf::from("."));
            DIR.get_or_init(|| DataDir { path: dot.clone(), base: dot, source: Source::Fallback, profile: None })
        }
    }
}

/// Das `--data-dir`-Argument (absolut), falls angegeben.
#[allow(dead_code)] // für Autostart/MCP-Registrierung (Integrationen)
pub fn data_dir_arg() -> Option<&'static Path> {
    crate::cli::get().data_dir.as_deref()
}

/// Argumente, mit denen ein weiterer Start (Autostart, MCP-Registrierung) dieselbe Instanz trifft, z. B.
/// `["--data-dir", "D:\\pd", "--profile", "work"]`; leer beim Standard-Datenordner. Ein über `PANELDECK_DATA`
/// gewählter Ordner wird ebenfalls als `--data-dir` festgeschrieben (die Variable fehlt z. B. im Autostart evtl.).
/// Genutzt von `launch::instance_args` (Autostart, Kontextmenü, MCP-Registrierung).
pub fn instance_args() -> Vec<String> {
    instance_args_for(get())
}

pub fn instance_args_for(d: &DataDir) -> Vec<String> {
    let mut v = Vec::new();
    if matches!(d.source, Source::Arg | Source::Env) {
        v.push("--data-dir".to_string());
        v.push(d.base.display().to_string());
    }
    if let Some(p) = &d.profile {
        v.push("--profile".to_string());
        v.push(p.clone());
    }
    v
}

/// Herkunft in Worten (Info-Seite, Diagnose).
pub fn source_label(s: Source, l: Lang) -> &'static str {
    match s {
        Source::Arg => l.t("festgelegt mit --data-dir", "set by --data-dir"),
        Source::Env => l.t("festgelegt mit PANELDECK_DATA", "set by PANELDECK_DATA"),
        Source::Portable => l.t("portabler Ordner neben der .exe", "portable folder next to the .exe"),
        Source::AppData => {
            if cfg!(windows) {
                "%APPDATA%"
            } else if cfg!(target_os = "macos") {
                "~/Library/Application Support"
            } else {
                l.t("Benutzerordner (XDG, ~/.local/share)", "user folder (XDG, ~/.local/share)")
            }
        }
        Source::Fallback => l.t("Notlösung (aktuelles Verzeichnis)", "fallback (current directory)"),
    }
}

/// Ordner, dessen `settings.json` die Sprache für Hilfe und Fehlermeldungen liefert – ohne etwas anzulegen.
pub fn peek_dir(inp: &Inputs) -> Option<PathBuf> {
    let base = if let Some(a) = &inp.arg {
        a.clone()
    } else if let Some(e) = &inp.env {
        e.clone()
    } else {
        let portable = inp.exe_dir.as_ref().map(|d| d.join("paneldeck-data")).filter(|p| p.is_dir());
        // (Linux/macOS: auch beim Start zählt der portable Ordner nur, wenn es ihn gibt – wie hier.)
        portable.or_else(|| inp.appdata.as_ref().map(|d| d.join("Paneldeck")))?
    };
    Some(match &inp.profile {
        Some(p) => base.join("profiles").join(p),
        None => base,
    })
}

/// Ordner, in dem die MCP-Brücke `ipc.json` sucht:
/// 1. `--data-dir` (mit oder ohne `--profile`) → wie beim Hub-Start;
/// 2. nur `--profile` aus einem Hub-Terminal → `<Basis dieses Hubs>\profiles\<name>` (`PANELDECK_HUB_BASE`);
/// 3. ohne Angabe aus einem Hub-Terminal → Datenordner dieses Hubs (`PANELDECK_HUB_DATA`);
/// 4. sonst die normale Auflösung.
pub fn bridge_dir_with(inp: &Inputs, hub_data: Option<PathBuf>, hub_base: Option<PathBuf>, usable: &dyn Fn(&Path) -> bool) -> Result<PathBuf, PathError> {
    if inp.arg.is_none() {
        match (&inp.profile, hub_base, hub_data) {
            (Some(_), Some(base), _) => {
                let i = Inputs { arg: Some(base), ..inp.clone() };
                return resolve_with(&i, usable).map(|d| d.path);
            }
            (None, _, Some(h)) => return Ok(h),
            _ => {}
        }
    }
    resolve_with(inp, usable).map(|d| d.path)
}

pub fn bridge_dir(cli: &Cli) -> Result<PathBuf, PathError> {
    bridge_dir_with(&inputs(cli), env_path(ENV_HUB_DATA), env_path(ENV_HUB_BASE), &usable)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Tmp {
            let p = std::env::temp_dir().join(format!("pd-paths-{tag}-{}-{:?}", std::process::id(), std::thread::current().id()).replace(['(', ')'], ""));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn inputs_in(t: &Path) -> Inputs {
        // Linux/macOS: der portable Ordner zählt nur, wenn es ihn gibt.
        let _ = std::fs::create_dir_all(t.join("exe").join("paneldeck-data"));
        Inputs { arg: None, env: None, exe_dir: Some(t.join("exe")), appdata: Some(t.join("appdata")), profile: None }
    }

    #[test]
    fn lookup_order() {
        let t = Tmp::new("order");
        let mut inp = inputs_in(&t.0);
        // portabel neben der .exe
        let d = resolve_with(&inp, &usable).unwrap();
        assert_eq!((d.path.clone(), d.source), (t.0.join("exe").join("paneldeck-data"), Source::Portable));
        assert!(instance_args_for(&d).is_empty());
        // portabel nicht beschreibbar (eine Datei versperrt den Ordner) → %APPDATA%
        std::fs::write(t.0.join("blocker"), b"x").unwrap();
        inp.exe_dir = Some(t.0.join("blocker"));
        let d = resolve_with(&inp, &usable).unwrap();
        assert_eq!((d.path.clone(), d.source), (t.0.join("appdata").join("Paneldeck"), Source::AppData));
        // beides unbrauchbar → Notlösung (absolut)
        inp.appdata = Some(t.0.join("blocker"));
        let d = resolve_with(&inp, &usable).unwrap();
        assert_eq!(d.source, Source::Fallback);
        assert!(d.path.is_absolute());
        // Umgebungsvariable schlägt portabel
        inp = inputs_in(&t.0);
        inp.env = Some(t.0.join("env"));
        let d = resolve_with(&inp, &usable).unwrap();
        assert_eq!((d.path.clone(), d.source), (t.0.join("env"), Source::Env));
        assert_eq!(instance_args_for(&d), vec!["--data-dir".to_string(), t.0.join("env").display().to_string()]);
        // Argument schlägt Umgebungsvariable
        inp.arg = Some(t.0.join("arg"));
        let d = resolve_with(&inp, &usable).unwrap();
        assert_eq!((d.path.clone(), d.source), (t.0.join("arg"), Source::Arg));
        assert!(t.0.join("arg").is_dir());
    }

    #[test]
    fn profiles_and_errors() {
        let t = Tmp::new("prof");
        let mut inp = inputs_in(&t.0);
        inp.profile = Some("work".into());
        let d = resolve_with(&inp, &usable).unwrap();
        assert_eq!(d.path, t.0.join("exe").join("paneldeck-data").join("profiles").join("work"));
        assert_eq!(d.base, t.0.join("exe").join("paneldeck-data"));
        assert!(d.path.is_dir());
        assert_eq!(instance_args_for(&d), vec!["--profile".to_string(), "work".to_string()]);
        inp.arg = Some(t.0.join("custom"));
        let d = resolve_with(&inp, &usable).unwrap();
        assert_eq!(d.path, t.0.join("custom").join("profiles").join("work"));
        assert_eq!(instance_args_for(&d), vec!["--data-dir".to_string(), t.0.join("custom").display().to_string(), "--profile".to_string(), "work".to_string()]);
        // unbrauchbares --data-dir / PANELDECK_DATA: Fehler statt Rückfall
        std::fs::write(t.0.join("file"), b"x").unwrap();
        inp.profile = None;
        inp.arg = Some(t.0.join("file").join("sub"));
        let e = resolve_with(&inp, &usable).unwrap_err();
        assert_eq!(e.source, Source::Arg);
        assert!(e.message(Lang::En).contains("--data-dir") && e.message(Lang::De).contains("--data-dir"));
        inp.arg = None;
        inp.env = Some(t.0.join("file"));
        assert_eq!(resolve_with(&inp, &usable).unwrap_err().source, Source::Env);
    }

    #[cfg(windows)] // Windows-Pfade (Laufwerksbuchstaben, `\`)
    #[test]
    fn bridge_and_peek() {
        let no = |_: &Path| false;
        let mut inp = Inputs { exe_dir: Some(PathBuf::from("C:\\nope-exe")), appdata: Some(PathBuf::from("C:\\ad")), ..Default::default() };
        let hub = || Some(PathBuf::from("D:\\hub\\profiles\\a"));
        let hub_base = || Some(PathBuf::from("D:\\hub"));
        // Terminal eines Hubs: dessen Ordner, ohne etwas anzulegen
        assert_eq!(bridge_dir_with(&inp, hub(), hub_base(), &no).unwrap(), PathBuf::from("D:\\hub\\profiles\\a"));
        // nur --profile im Hub-Terminal: Profil unter der Basis dieses Hubs
        inp.profile = Some("p1".into());
        let yes = |_: &Path| true;
        assert_eq!(bridge_dir_with(&inp, hub(), hub_base(), &yes).unwrap(), PathBuf::from("D:\\hub\\profiles\\p1"));
        // nur --profile außerhalb eines Hub-Terminals: normale Auflösung
        assert_eq!(bridge_dir_with(&inp, None, None, &yes).unwrap(), PathBuf::from("C:\\nope-exe\\paneldeck-data\\profiles\\p1"));
        // --data-dir schlägt den Terminal-Hub
        inp.profile = None;
        inp.arg = Some(PathBuf::from("D:\\x"));
        assert_eq!(bridge_dir_with(&inp, hub(), hub_base(), &yes).unwrap(), PathBuf::from("D:\\x"));
        // peek legt nichts an und nimmt ohne portablen Ordner %APPDATA%
        inp.arg = None;
        assert_eq!(peek_dir(&inp), Some(PathBuf::from("C:\\ad\\Paneldeck")));
        inp.profile = Some("w".into());
        inp.env = Some(PathBuf::from("E:\\e"));
        assert_eq!(peek_dir(&inp), Some(PathBuf::from("E:\\e\\profiles\\w")));
    }

    #[test]
    fn portable_folder_only_if_present_off_windows() {
        let t = Tmp::new("present");
        let inp = Inputs { arg: None, env: None, exe_dir: Some(t.0.join("bin")), appdata: Some(t.0.join("share")), profile: None };
        std::fs::create_dir_all(t.0.join("bin")).unwrap();
        let d = resolve_with(&inp, &usable).unwrap();
        if cfg!(windows) {
            assert_eq!(d.source, Source::Portable);
        } else {
            assert_eq!((d.path.clone(), d.source), (t.0.join("share").join("Paneldeck"), Source::AppData));
            assert!(!t.0.join("bin").join("paneldeck-data").exists(), "nothing created next to the binary");
        }
    }

    #[cfg(unix)]
    #[test]
    fn data_folder_and_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let t = Tmp::new("private");
        let dir = t.0.join("data");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        harden(&DataDir { path: dir.clone(), base: dir.clone(), source: Source::AppData, profile: None });
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        let f = dir.join("ipc.json");
        std::fs::write(&f, b"old").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        crate::state::write_atomic(&f, b"{}").unwrap();
        assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::read(&f).unwrap(), b"{}");
    }

    #[cfg(unix)] // Unix-Pfade
    #[test]
    fn xdg_rules() {
        let home = Some(PathBuf::from("/home/a"));
        assert_eq!(xdg_data_home(None, home.clone()), Some(PathBuf::from("/home/a/.local/share")));
        assert_eq!(xdg_data_home(Some(PathBuf::from("/data/x")), home.clone()), Some(PathBuf::from("/data/x")));
        // relative values are invalid per spec → default
        assert_eq!(xdg_data_home(Some(PathBuf::from("rel")), home.clone()), Some(PathBuf::from("/home/a/.local/share")));
        assert_eq!(xdg_data_home(None, None), None);
    }

    #[test]
    fn labels_in_both_languages() {
        for s in [Source::Arg, Source::Env, Source::Portable, Source::Fallback] {
            assert_ne!(source_label(s, Lang::En), source_label(s, Lang::De));
        }
        assert!(source_label(Source::Arg, Lang::En).contains("--data-dir"));
    }
}
