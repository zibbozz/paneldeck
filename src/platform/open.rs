//! Open things with the system: files/URLs with their default program, folders and items in the file manager, web
//! pages in the browser. The callers check targets first (`links`, `app::update::release::safe_release_page`).

use std::path::Path;

/// Does this platform have a file manager we can open (shows "Show in Explorer"/"Open folder" buttons)?
/// Linux (#149): the desktop's file manager through `xdg-open`; macOS (#150): Finder through `open`.
pub const FILE_MANAGER: bool = true;

/// Open `target` (URL, file or folder) with its default program. `false` if nothing could be started.
pub fn launch(target: &str) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", target])
            .creation_flags(0x0800_0000)
            .spawn()
            .is_ok()
    }
    #[cfg(target_os = "macos")]
    {
        // a bundle folder (X.app …) is only shown, never started
        let p = Path::new(target);
        let reveal = !target.contains("://") && p.is_dir() && is_bundle_like(p);
        mac_open(if reveal { &["-R"] } else { &[] }, std::ffi::OsStr::new(target))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        xdg_open(std::ffi::OsStr::new(target))
    }
}

/// macOS (#150): `open [flags] <target>` in the background and waited for (no zombie). A path that starts with `-` would
/// be read as an option: it is made `./-…`. `false` if it could not be started.
#[cfg(target_os = "macos")]
fn mac_open(flags: &[&str], target: &std::ffi::OsStr) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let arg: std::ffi::OsString = if target.as_bytes().starts_with(b"-") {
        let mut s = std::ffi::OsString::from("./");
        s.push(target);
        s
    } else {
        target.to_owned()
    };
    match std::process::Command::new("/usr/bin/open").args(flags).arg(&arg).stdin(std::process::Stdio::null()).spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            true
        }
        Err(_) => false,
    }
}

/// Show `target` selected in its folder (used for executable files instead of running them). The caller has checked
/// the path (`links::reveal_safe`: Explorer reads switches from the argument).
pub fn reveal(target: &str) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("explorer.exe").arg(format!("/select,{target}")).creation_flags(0x0800_0000).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        mac_open(&["-R"], std::ffi::OsStr::new(target));
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        if let Some(dir) = Path::new(target).parent() {
            xdg_open(dir.as_os_str());
        }
    }
}

/// Linux: `xdg-open` in the background and waited for (no zombie). A path that starts with `-` would be read as an
/// option: it is made `./-…`. `false` if it could not be started.
#[cfg(not(any(windows, target_os = "macos")))]
fn xdg_open(target: &std::ffi::OsStr) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let arg: std::ffi::OsString = if target.as_bytes().starts_with(b"-") {
        let mut s = std::ffi::OsString::from("./");
        s.push(target);
        s
    } else {
        target.to_owned()
    };
    match std::process::Command::new("xdg-open").arg(&arg).stdin(std::process::Stdio::null()).spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            true
        }
        Err(_) => false,
    }
}

/// Open a folder in the file manager. Only where `FILE_MANAGER` is true; elsewhere nothing happens.
pub fn folder(dir: &Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(dir).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        xdg_open(dir.as_os_str());
    }
    #[cfg(target_os = "macos")]
    {
        mac_open(if is_bundle_like(dir) { &["-R"] } else { &[] }, dir.as_os_str());
    }
}

/// macOS: ein Ordner mit Endung (`X.app`, `Y.prefPane`, `Z.workflow` …) ist ein Bündel – `open` würde es starten bzw.
/// installieren. Solche Ordner werden nur im Finder markiert (`open -R`).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn is_bundle_like(p: &Path) -> bool {
    p.extension().is_some_and(|e| !e.is_empty())
}

/// File tree: a folder is opened, a file is shown selected in its folder. Only where `FILE_MANAGER` is true.
pub fn show_in_file_manager(path: &str, is_dir: bool) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(if is_dir { path.to_string() } else { format!("/select,{path}") }).spawn();
    }
    // Linux: a folder is opened, for a file its folder (xdg-open has no "select").
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let p = Path::new(path);
        let target = if is_dir { Some(p) } else { p.parent() };
        if let Some(t) = target {
            xdg_open(t.as_os_str());
        }
    }
    // macOS: a folder is opened in Finder, a file or a bundle (`X.app` is a folder too – `open` would start it) is shown
    // selected (`open -R`).
    #[cfg(target_os = "macos")]
    {
        let reveal = !is_dir || is_bundle_like(Path::new(path));
        mac_open(if reveal { &["-R"] } else { &[] }, std::ffi::OsStr::new(path));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn bundles_are_recognised() {
        use std::path::Path;
        assert!(super::is_bundle_like(Path::new("/Applications/Calculator.app")));
        assert!(super::is_bundle_like(Path::new("/x/Y.prefPane/")));
        assert!(!super::is_bundle_like(Path::new("/Users/a/projects")));
    }
}

/// Open a web page (already checked by the caller) in the default browser. Windows: `ShellExecuteW` (no detour through
/// `explorer`). Elsewhere: `launch`.
pub fn url(url: &str) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let op: Vec<u16> = "open".encode_utf16().chain([0]).collect();
        let u: Vec<u16> = url.encode_utf16().chain([0]).collect();
        // SAFETY: nullterminierte UTF-16-Puffer, die den Aufruf überleben; kein Fenster-Handle nötig.
        unsafe { ShellExecuteW(std::ptr::null_mut(), op.as_ptr(), u.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) };
    }
    #[cfg(not(windows))]
    launch(url);
}
