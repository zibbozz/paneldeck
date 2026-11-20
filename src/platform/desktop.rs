//! Linux desktop integration (#149), written into the user's folders only (no root, works for every package format):
//!
//! * **Autostart:** an XDG autostart entry `~/.config/autostart/<name>.desktop` (`<name>` = `paneldeck` or
//!   `paneldeck-<instance>`, like the MCP server name of the instance), `Exec=… --tray`. Off by default.
//! * **"Open in Paneldeck" in the file manager** (#149 decision: Nautilus, Dolphin, Nemo), off by default; turning it
//!   on writes the files below, turning it off removes them (only files that carry the Paneldeck marker):
//!   - Nautilus: scripts in `~/.local/share/nautilus/scripts/` (right-click → Scripts),
//!   - Dolphin: a service menu in `~/.local/share/kio/servicemenus/` (KF6) and `~/.local/share/kservices5/ServiceMenus/`
//!     (KF5),
//!   - Nemo: actions in `~/.local/share/nemo/actions/`.
//!
//!   Like the Explorer context menu on Windows there is one set for all instances; the instance that writes last is in
//!   the command.
//!
//! What is written is a pure plan ([`FileOp`], tested); [`apply`] carries it out.

use std::path::{Path, PathBuf};

/// Marker in every file Paneldeck writes; only files with it are removed again.
pub const MARKER: &str = "X-Paneldeck-Integration";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileOp {
    pub path: PathBuf,
    /// `None` = remove (if it carries the marker).
    pub content: Option<String>,
    /// Make it executable (scripts, KF6 service menus).
    pub exec: bool,
}

/// The program to put into the entries: the `.AppImage` file itself (the running binary lives in a temporary mount),
/// otherwise this executable.
pub fn this_exe() -> PathBuf {
    if let Some(a) = std::env::var_os("APPIMAGE").filter(|a| !a.is_empty()) {
        return PathBuf::from(a);
    }
    std::env::current_exe().unwrap_or_default()
}

/// One argument for the `Exec` key of a desktop entry: quoted in double quotes when it contains reserved characters
/// (`"`, `` ` ``, `$`, `\` escaped inside), `%` doubled. Control characters are refused.
pub fn exec_arg(a: &str) -> Result<String, String> {
    if a.chars().any(|c| c.is_control()) {
        return Err(crate::state::t("Ungültiges Zeichen im Pfad", "Invalid character in the path").to_string());
    }
    let a = a.replace('%', "%%");
    let reserved = |c: char| " \t\n\"'\\><~|&;$*?#()`".contains(c);
    if !a.is_empty() && !a.chars().any(reserved) {
        return Ok(a);
    }
    let mut out = String::from("\"");
    for c in a.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    // The value of a desktop-entry key is itself an escaped string: a literal backslash is written as `\\`.
    Ok(out.replace('\\', "\\\\"))
}

/// `Exec` line: program, instance arguments, then `extra` (already in their final form, e.g. `%f`).
pub fn exec_line(exe: &Path, args: &[String], extra: &[&str]) -> Result<String, String> {
    let mut parts = vec![exec_arg(&exe.to_string_lossy())?];
    for a in args {
        parts.push(exec_arg(a)?);
    }
    parts.extend(extra.iter().map(|s| s.to_string()));
    Ok(parts.join(" "))
}

/// Single-quote a word for `sh`.
pub fn sh_quote(a: &str) -> String {
    format!("'{}'", a.replace('\'', "'\\''"))
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from)
}

fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).or_else(|| home().map(|h| h.join(".config")))
}

fn data_home() -> Option<PathBuf> {
    crate::paths::xdg_data_home(std::env::var_os("XDG_DATA_HOME").map(PathBuf::from), home())
}

/// File of the autostart entry of this instance.
pub fn autostart_file(config: &Path, args: &[String]) -> PathBuf {
    let name = crate::launch::mcp_server_name(&crate::launch::Instance::from_args(args));
    config.join("autostart").join(format!("{name}.desktop"))
}

pub fn autostart_plan(on: bool, config: &Path, exe: &Path, args: &[String]) -> Result<Vec<FileOp>, String> {
    let path = autostart_file(config, args);
    if !on {
        return Ok(vec![FileOp { path, content: None, exec: false }]);
    }
    let exec = exec_line(exe, args, &["--tray"])?;
    let content = format!(
        "[Desktop Entry]\nType=Application\nName=Paneldeck\nComment={}\nExec={exec}\nIcon=paneldeck\nTerminal=false\nX-GNOME-Autostart-enabled=true\n{MARKER}=autostart\n",
        crate::state::t("Terminal-Manager (minimiert im Tray)", "Terminal manager (minimized in the tray)")
    );
    Ok(vec![FileOp { path, content: Some(content), exec: false }])
}

/// `Exec=` value of a desktop entry (first one).
pub fn exec_of(content: &str) -> Option<&str> {
    content.lines().find_map(|l| l.strip_prefix("Exec="))
}

/// Labels in the current language (the menu entries are written in the saved language, like on Windows).
fn labels() -> (&'static str, &'static str) {
    (crate::state::t("In Paneldeck öffnen", "Open in Paneldeck"), crate::state::t("Mit Claude in Paneldeck starten", "Start with Claude in Paneldeck"))
}

/// Every label any language may have used (to remove Nautilus scripts written in the other language).
const ALL_LABELS: [&str; 4] = ["In Paneldeck öffnen", "Open in Paneldeck", "Mit Claude in Paneldeck starten", "Start with Claude in Paneldeck"];

fn nautilus_script(exe: &Path, args: &[String], claude: bool) -> String {
    let mut cmd = vec![sh_quote(&exe.to_string_lossy())];
    cmd.extend(args.iter().map(|a| sh_quote(a)));
    if claude {
        cmd.push("--claude".into());
    }
    format!(
        "#!/bin/sh\n# {MARKER}: Paneldeck file-manager integration (Settings → Desktop integration).\n# Nautilus runs scripts in the current folder; a selected folder wins.\nd=\"$PWD\"\nfor f in \"$@\"; do\n  if [ -d \"$f\" ]; then d=\"$f\"; break; fi\ndone\nexec {} --open \"$d\"\n",
        cmd.join(" ")
    )
}

fn dolphin_menu(exe: &Path, args: &[String]) -> Result<String, String> {
    let (open, claude) = labels();
    Ok(format!(
        "[Desktop Entry]\nType=Service\nMimeType=inode/directory;\nActions=paneldeckOpen;paneldeckClaude;\nX-KDE-Priority=TopLevel\n{MARKER}=dolphin\n\n[Desktop Action paneldeckOpen]\nName={open}\nIcon=utilities-terminal\nExec={}\n\n[Desktop Action paneldeckClaude]\nName={claude}\nIcon=utilities-terminal\nExec={}\n",
        exec_line(exe, args, &["--open", "%f"])?,
        exec_line(exe, args, &["--claude", "--open", "%f"])?
    ))
}

fn nemo_action(exe: &Path, args: &[String], claude: bool, background: bool) -> Result<String, String> {
    // Nemo expands its own %-tokens and does not know `%%`: refuse a literal `%` instead of guessing.
    if exe.to_string_lossy().contains('%') || args.iter().any(|a| a.contains('%')) {
        return Err(crate::state::t("„%“ im Programmpfad oder in den Argumenten wird von Nemo nicht unterstützt", "“%” in the program path or the arguments is not supported by Nemo").to_string());
    }
    let (open, with_claude) = labels();
    let name = if claude { with_claude } else { open };
    let mut extra: Vec<&str> = Vec::new();
    if claude {
        extra.push("--claude");
    }
    extra.push("--open");
    extra.push(if background { "%P" } else { "%F" });
    let selection = if background { "None" } else { "s" };
    Ok(format!(
        "[Nemo Action]\nName={name}\nComment={name}\nExec={}\nIcon-Name=utilities-terminal\nSelection={selection}\nExtensions=dir;\n{MARKER}=nemo\n",
        exec_line(exe, args, &extra)?
    ))
}

const NEMO_FILES: [(&str, bool, bool); 4] = [
    ("paneldeck-open.nemo_action", false, false),
    ("paneldeck-open-here.nemo_action", false, true),
    ("paneldeck-claude.nemo_action", true, false),
    ("paneldeck-claude-here.nemo_action", true, true),
];

/// Dolphin service-menu files (KF6, KF5) under the data home.
pub fn dolphin_files(data: &Path) -> [PathBuf; 2] {
    [data.join("kio").join("servicemenus").join("paneldeck.desktop"), data.join("kservices5").join("ServiceMenus").join("paneldeck.desktop")]
}

pub fn context_plan(on: bool, data: &Path, exe: &Path, args: &[String]) -> Result<Vec<FileOp>, String> {
    let mut ops = Vec::new();
    let scripts = data.join("nautilus").join("scripts");
    let (open, claude) = labels();
    for label in ALL_LABELS {
        // the scripts of the current language are (re)written below; the others go away
        if !on || (label != open && label != claude) {
            ops.push(FileOp { path: scripts.join(label), content: None, exec: false });
        }
    }
    if on {
        ops.push(FileOp { path: scripts.join(open), content: Some(nautilus_script(exe, args, false)), exec: true });
        ops.push(FileOp { path: scripts.join(claude), content: Some(nautilus_script(exe, args, true)), exec: true });
    }
    let menu = if on { Some(dolphin_menu(exe, args)?) } else { None };
    for f in dolphin_files(data) {
        ops.push(FileOp { path: f, content: menu.clone(), exec: true });
    }
    let actions = data.join("nemo").join("actions");
    for (file, claude, background) in NEMO_FILES {
        let content = if on { Some(nemo_action(exe, args, claude, background)?) } else { None };
        ops.push(FileOp { path: actions.join(file), content, exec: false });
    }
    Ok(ops)
}

/// Carry out a plan. A file to remove is only removed if it carries [`MARKER`], and a file is only (over)written if it
/// does not exist yet or carries the marker (a file of the user stays untouched).
pub fn apply(ops: &[FileOp]) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    for op in ops {
        match &op.content {
            Some(c) => {
                // never overwrite a file of the user (same name, without our marker)
                if std::fs::symlink_metadata(&op.path).is_ok() && !std::fs::read_to_string(&op.path).is_ok_and(|x| x.contains(MARKER)) {
                    return Err(format!("{}: {}", op.path.display(), crate::state::t("Datei gibt es schon (nicht von Paneldeck) – nicht überschrieben", "file exists (not from Paneldeck) – not overwritten")));
                }
                if let Some(dir) = op.path.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                }
                crate::state::write_atomic(&op.path, c.as_bytes()).map_err(|e| format!("{}: {e}", op.path.display()))?;
                let mode = if op.exec { 0o755 } else { 0o644 };
                std::fs::set_permissions(&op.path, std::fs::Permissions::from_mode(mode)).map_err(|e| format!("{}: {e}", op.path.display()))?;
            }
            None => {
                if std::fs::read_to_string(&op.path).is_ok_and(|c| c.contains(MARKER)) {
                    std::fs::remove_file(&op.path).map_err(|e| format!("{}: {e}", op.path.display()))?;
                }
            }
        }
    }
    Ok(())
}

fn need(p: Option<PathBuf>) -> Result<PathBuf, String> {
    p.ok_or_else(|| crate::state::t("HOME ist nicht gesetzt", "HOME is not set").to_string())
}

/// Stored `Exec` of this instance's autostart entry (`None` = no entry).
pub fn autostart_stored(args: &[String]) -> Option<String> {
    let f = autostart_file(&config_home()?, args);
    exec_of(&std::fs::read_to_string(f).ok()?).map(str::to_string)
}

pub fn autostart_expected(args: &[String]) -> Result<String, String> {
    exec_line(&this_exe(), args, &["--tray"])
}

pub fn set_autostart(on: bool, args: &[String]) -> Result<(), String> {
    apply(&autostart_plan(on, &need(config_home())?, &this_exe(), args)?)
}

/// Stored "open" command of the file-manager integration (from the Dolphin menu, else a Nemo action).
pub fn context_stored() -> Option<String> {
    let data = data_home()?;
    let mut files: Vec<PathBuf> = dolphin_files(&data).into();
    files.push(data.join("nemo").join("actions").join(NEMO_FILES[0].0));
    files.into_iter().find_map(|f| exec_of(&std::fs::read_to_string(f).ok()?).map(str::to_string))
}

pub fn context_expected(args: &[String]) -> Result<String, String> {
    exec_line(&this_exe(), args, &["--open", "%f"])
}

pub fn set_context(on: bool, args: &[String]) -> Result<(), String> {
    apply(&context_plan(on, &need(data_home())?, &this_exe(), args)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn exec_quoting() {
        assert_eq!(exec_arg("/usr/bin/paneldeck").unwrap(), "/usr/bin/paneldeck");
        assert_eq!(exec_arg("/home/a b/Paneldeck.AppImage").unwrap(), "\"/home/a b/Paneldeck.AppImage\"");
        assert_eq!(exec_arg("100%").unwrap(), "100%%");
        assert_eq!(exec_arg("a\"$b").unwrap(), "\"a\\\\\"\\\\$b\"");
        assert!(exec_arg("x\ny").is_err());
        assert_eq!(exec_arg("").unwrap(), "\"\"");
        assert_eq!(sh_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn autostart_entries() {
        let cfg = PathBuf::from("/home/a/.config");
        let exe = PathBuf::from("/opt/Paneldeck/paneldeck");
        let on = autostart_plan(true, &cfg, &exe, &[]).unwrap();
        assert_eq!(on[0].path, PathBuf::from("/home/a/.config/autostart/paneldeck.desktop"));
        let c = on[0].content.as_deref().unwrap();
        assert_eq!(exec_of(c), Some("/opt/Paneldeck/paneldeck --tray"));
        assert!(c.contains(MARKER) && c.starts_with("[Desktop Entry]\n"));
        let w = autostart_plan(true, &cfg, &exe, &s(&["--profile", "Work"])).unwrap();
        assert_eq!(w[0].path, PathBuf::from("/home/a/.config/autostart/paneldeck-work.desktop"));
        assert_eq!(exec_of(w[0].content.as_deref().unwrap()), Some("/opt/Paneldeck/paneldeck --profile Work --tray"));
        assert_eq!(autostart_plan(false, &cfg, &exe, &[]).unwrap()[0].content, None);
    }

    #[test]
    fn file_manager_entries() {
        let data = PathBuf::from("/home/a/.local/share");
        let exe = PathBuf::from("/home/a/Apps/Paneldeck x.AppImage");
        let on = context_plan(true, &data, &exe, &[]).unwrap();
        let get = |p: &str| on.iter().find(|o| o.path == data.join(p)).and_then(|o| o.content.clone());
        let dolphin = get("kio/servicemenus/paneldeck.desktop").unwrap();
        assert!(dolphin.contains("Exec=\"/home/a/Apps/Paneldeck x.AppImage\" --open %f\n"));
        assert!(dolphin.contains("Exec=\"/home/a/Apps/Paneldeck x.AppImage\" --claude --open %f\n"));
        assert_eq!(get("kservices5/ServiceMenus/paneldeck.desktop"), Some(dolphin));
        let nemo_here = get("nemo/actions/paneldeck-open-here.nemo_action").unwrap();
        assert!(nemo_here.contains("--open %P") && nemo_here.contains("Selection=None"));
        let (open, _) = labels();
        let script = get(&format!("nautilus/scripts/{open}")).unwrap();
        assert!(script.starts_with("#!/bin/sh\n") && script.contains("exec '/home/a/Apps/Paneldeck x.AppImage' --open \"$d\""));
        assert!(on.iter().filter(|o| o.content.is_some()).all(|o| o.content.as_deref().unwrap().contains(MARKER)));
        // scripts and KF6 menus are executable
        assert!(on.iter().filter(|o| o.path.to_string_lossy().contains("scripts") && o.content.is_some()).all(|o| o.exec));
        let off = context_plan(false, &data, &exe, &[]).unwrap();
        assert!(off.iter().all(|o| o.content.is_none()));
        assert_eq!(off.len(), 4 + 2 + 4);
    }

    #[test]
    fn apply_writes_and_removes_only_marked_files() {
        let t = std::env::temp_dir().join(format!("pd-desktop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        let exe = PathBuf::from("/usr/bin/paneldeck");
        apply(&context_plan(true, &t, &exe, &[]).unwrap()).unwrap();
        let script = t.join("nautilus/scripts").join(labels().0);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&script).unwrap().permissions().mode() & 0o777, 0o755);
        // a foreign file with the same name stays
        let foreign = t.join("nemo/actions/paneldeck-open.nemo_action");
        std::fs::write(&foreign, "[Nemo Action]\nName=mine\n").unwrap();
        apply(&context_plan(false, &t, &exe, &[]).unwrap()).unwrap();
        assert!(!script.exists());
        assert!(!t.join("kio/servicemenus/paneldeck.desktop").exists());
        assert!(foreign.exists(), "files without the marker are never removed");
        // … and never overwritten
        assert!(apply(&context_plan(true, &t, &exe, &[]).unwrap()).is_err());
        assert_eq!(std::fs::read_to_string(&foreign).unwrap(), "[Nemo Action]\nName=mine\n");
        assert!(context_plan(true, &t, &PathBuf::from("/opt/100%/paneldeck"), &[]).is_err(), "Nemo cannot quote %");
        let _ = std::fs::remove_dir_all(&t);
    }
}
