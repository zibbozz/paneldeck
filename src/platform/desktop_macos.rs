//! macOS desktop integration (#150), written into the user's folders only (no admin rights):
//!
//! * **Autostart:** a LaunchAgent `~/Library/LaunchAgents/io.github.zibbozz.<name>.plist` (`<name>` = `paneldeck` or
//!   `paneldeck-<instance>`, like the MCP server name of the instance) with `RunAtLoad` and `… --tray`. launchd reads it
//!   at the next login; nothing is loaded or unloaded now (`launchctl bootout` would stop the running hub). Off by
//!   default. macOS 13+ shows its own notice "Background item added" and lists it under *System Settings → General →
//!   Login Items*.
//! * **"Open in Paneldeck" in Finder:** not written by Paneldeck in 2.5.0 (a Quick Action/Automator workflow cannot be
//!   tested without a Mac). The docs describe a Quick Action the user can create in two minutes; the switch in the
//!   settings is replaced by that hint.
//!
//! The file content is a pure function ([`autostart_plist`], tested on every platform); [`apply`] writes or removes it
//! and never touches a file without the Paneldeck marker.

use std::path::{Path, PathBuf};

/// Marker word in every file Paneldeck writes (inside [`MARKER_LINE`]); only files with exactly that line count.
#[cfg_attr(not(test), allow(dead_code))]
pub const MARKER: &str = "X-Paneldeck-Integration";

/// The whole marker line (an XML comment) of a LaunchAgent written by Paneldeck.
pub const MARKER_LINE: &str = "<!-- X-Paneldeck-Integration: Paneldeck autostart (Settings → Desktop integration). -->";

/// Is this a file Paneldeck wrote (the exact marker line, not just the word somewhere)?
pub fn is_ours(content: &str) -> bool {
    content.lines().any(|l| l.trim_end_matches('\r') == MARKER_LINE)
}

/// Does this program run from a place that changes or vanishes (App Translocation, a mounted disk image)? Then an
/// autostart entry would point to a path that does not exist at the next login.
pub fn transient_location(exe: &Path) -> bool {
    let s = exe.to_string_lossy();
    s.contains("/AppTranslocation/") || s.starts_with("/Volumes/")
}

/// The program to put into the entries: this executable (inside `Paneldeck.app/Contents/MacOS/`).
pub fn this_exe() -> PathBuf {
    std::env::current_exe().unwrap_or_default()
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from)
}

/// launchd label of this instance's autostart job.
pub fn label(args: &[String]) -> String {
    format!("io.github.zibbozz.{}", crate::launch::mcp_server_name(&crate::launch::Instance::from_args(args)))
}

/// File of the LaunchAgent of this instance.
pub fn autostart_file(home: &Path, args: &[String]) -> PathBuf {
    home.join("Library").join("LaunchAgents").join(format!("{}.plist", label(args)))
}

/// Escape text for an XML `<string>`. Control characters (other than tab/newline) are refused – plists cannot hold them.
pub fn xml_escape(s: &str) -> Result<String, String> {
    if s.chars().any(|c| c.is_control() && c != '\t' && c != '\n') {
        return Err(crate::state::t("Ungültiges Zeichen im Pfad", "Invalid character in the path").to_string());
    }
    Ok(s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;"))
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// Program arguments of the autostart job: program, instance arguments, `--tray`.
pub fn autostart_args(exe: &Path, args: &[String]) -> Vec<String> {
    let mut v = vec![exe.to_string_lossy().into_owned()];
    v.extend(args.iter().cloned());
    v.push("--tray".into());
    v
}

/// The LaunchAgent property list.
pub fn autostart_plist(exe: &Path, args: &[String]) -> Result<String, String> {
    let mut items = String::new();
    for a in autostart_args(exe, args) {
        items.push_str(&format!("        <string>{}</string>\n", xml_escape(&a)?));
    }
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n{MARKER_LINE}\n<plist version=\"1.0\">\n<dict>\n    <key>Label</key>\n    <string>{}</string>\n    <key>ProgramArguments</key>\n    <array>\n{items}    </array>\n    <key>RunAtLoad</key>\n    <true/>\n    <key>ProcessType</key>\n    <string>Interactive</string>\n    <key>LimitLoadToSessionType</key>\n    <string>Aqua</string>\n</dict>\n</plist>\n",
        xml_escape(&label(args))?
    ))
}

/// `ProgramArguments` of a stored LaunchAgent (`None` = not found).
pub fn program_args_of(plist: &str) -> Option<Vec<String>> {
    let start = plist.find("<key>ProgramArguments</key>")?;
    let rest = &plist[start..];
    let arr = &rest[rest.find("<array>")? + 7..];
    let arr = &arr[..arr.find("</array>")?];
    let mut out = Vec::new();
    let mut s = arr;
    while let Some(i) = s.find("<string>") {
        let after = &s[i + 8..];
        let end = after.find("</string>")?;
        out.push(xml_unescape(&after[..end]));
        s = &after[end + 9..];
    }
    Some(out)
}

/// One line for the status comparison and the "points to another Paneldeck" hint (words single-quoted for `sh`).
pub fn args_line(v: &[String]) -> String {
    v.iter().map(|a| format!("'{}'", a.replace('\'', "'\\''"))).collect::<Vec<_>>().join(" ")
}

/// Write (`Some`) or remove (`None`) a file. Writing only replaces a missing file or one with [`MARKER_LINE`]; removing
/// only removes a file with it (a file of the user stays untouched).
pub fn apply(path: &Path, content: Option<&str>) -> Result<(), String> {
    let ours = || std::fs::read_to_string(path).is_ok_and(|c| is_ours(&c));
    match content {
        Some(c) => {
            if std::fs::symlink_metadata(path).is_ok() && !ours() {
                return Err(format!("{}: {}", path.display(), crate::state::t("Datei gibt es schon (nicht von Paneldeck) – nicht überschrieben", "file exists (not from Paneldeck) – not overwritten")));
            }
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            crate::state::write_atomic(path, c.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
        }
        None => {
            if ours() {
                std::fs::remove_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
            }
            Ok(())
        }
    }
}

fn need_home() -> Result<PathBuf, String> {
    home().ok_or_else(|| crate::state::t("HOME ist nicht gesetzt", "HOME is not set").to_string())
}

/// Stored program arguments of this instance's LaunchAgent (`None` = no entry).
pub fn autostart_stored(args: &[String]) -> Option<String> {
    let f = autostart_file(&home()?, args);
    program_args_of(&std::fs::read_to_string(f).ok()?).map(|v| args_line(&v))
}

pub fn autostart_expected(args: &[String]) -> Result<String, String> {
    Ok(args_line(&autostart_args(&this_exe(), args)))
}

pub fn set_autostart(on: bool, args: &[String]) -> Result<(), String> {
    let path = autostart_file(&need_home()?, args);
    if on && transient_location(&this_exe()) {
        return Err(crate::state::t(
            "Paneldeck läuft aus dem Disk-Image oder einem vorübergehenden Ort (App Translocation) – bitte zuerst Paneldeck.app nach „Programme“ ziehen und von dort starten.",
            "Paneldeck runs from the disk image or a temporary location (App Translocation) – please drag Paneldeck.app to Applications first and start it from there.",
        )
        .to_string());
    }
    if on {
        apply(&path, Some(&autostart_plist(&this_exe(), args)?))
    } else {
        apply(&path, None)
    }
}

/// Finder integration is not written by Paneldeck (see the module docs): always "off".
pub fn context_stored() -> Option<String> {
    None
}

pub fn context_expected(_args: &[String]) -> Result<String, String> {
    Err(crate::platform::unsupported())
}

pub fn set_context(_on: bool, _args: &[String]) -> Result<(), String> {
    Err(crate::platform::unsupported())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn launch_agent_content_and_round_trip() {
        let exe = PathBuf::from("/Applications/Paneldeck.app/Contents/MacOS/paneldeck");
        let p = autostart_plist(&exe, &[]).unwrap();
        assert!(p.contains("<string>io.github.zibbozz.paneldeck</string>") && p.contains(MARKER) && p.contains("<key>RunAtLoad</key>\n    <true/>"));
        assert_eq!(program_args_of(&p).unwrap(), ["/Applications/Paneldeck.app/Contents/MacOS/paneldeck", "--tray"]);
        let args = s(&["--data-dir", "/Users/a/My <Data> & more", "--profile", "Work"]);
        let w = autostart_plist(&exe, &args).unwrap();
        assert!(w.contains("&lt;Data&gt; &amp; more"));
        assert_eq!(program_args_of(&w).unwrap(), autostart_args(&exe, &args), "escaping round-trips");
        assert_eq!(autostart_file(Path::new("/Users/a"), &s(&["--profile", "Work"])), PathBuf::from("/Users/a/Library/LaunchAgents/io.github.zibbozz.paneldeck-work.plist"));
        assert!(autostart_plist(&PathBuf::from("/x\u{1}y"), &[]).is_err());
        assert_eq!(args_line(&s(&["/a b/it's", "--tray"])), "'/a b/it'\\''s' '--tray'");
        assert_eq!(program_args_of("<plist/>"), None);
    }

    #[test]
    fn apply_respects_files_of_the_user() {
        let d = std::env::temp_dir().join(format!("pd-launchagent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let f = d.join("LaunchAgents").join("io.github.zibbozz.paneldeck.plist");
        let c = autostart_plist(Path::new("/Applications/Paneldeck.app/Contents/MacOS/paneldeck"), &[]).unwrap();
        apply(&f, Some(&c)).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), c);
        apply(&f, Some(&c)).unwrap(); // own file: rewritten
        apply(&f, None).unwrap();
        assert!(!f.exists());
        std::fs::write(&f, "<plist>mine</plist>").unwrap();
        assert!(apply(&f, Some(&c)).is_err(), "never overwritten");
        apply(&f, None).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "<plist>mine</plist>", "never removed");
        // the word alone (e.g. in a comment of the user) is not our marker
        let theirs = format!("<!-- copied from {MARKER} -->\n<plist>mine</plist>");
        std::fs::write(&f, &theirs).unwrap();
        assert!(!is_ours(&theirs) && is_ours(&c));
        assert!(apply(&f, Some(&c)).is_err());
        apply(&f, None).unwrap();
        assert!(f.exists());
        assert!(transient_location(Path::new("/private/var/folders/x/T/AppTranslocation/ABC/d/Paneldeck.app/Contents/MacOS/paneldeck")));
        assert!(transient_location(Path::new("/Volumes/Paneldeck 2.5.0/Paneldeck.app/Contents/MacOS/paneldeck")));
        assert!(!transient_location(Path::new("/Applications/Paneldeck.app/Contents/MacOS/paneldeck")));
        let _ = std::fs::remove_dir_all(&d);
    }
}
