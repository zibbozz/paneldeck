//! Open things with the system: files/URLs with their default program, folders and items in the file manager, web
//! pages in the browser.

use std::path::Path;

/// Does this platform have a file manager we can open (shows "Show in Explorer"/"Open folder" buttons)?
pub const FILE_MANAGER: bool = cfg!(windows);

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
    #[cfg(not(windows))]
    {
        std::process::Command::new("xdg-open").arg(target).spawn().is_ok()
    }
}

/// Show `target` selected in its folder (used for executable files instead of running them).
pub fn reveal(target: &str) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("explorer.exe").arg(format!("/select,{target}")).creation_flags(0x0800_0000).spawn();
    }
    #[cfg(not(windows))]
    {
        if let Some(dir) = Path::new(target).parent() {
            let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
        }
    }
}

/// Open a folder in the file manager. Only where `FILE_MANAGER` is true; elsewhere nothing happens.
pub fn folder(dir: &Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(dir).spawn();
    }
    #[cfg(not(windows))]
    let _ = dir;
}

/// File tree: a folder is opened, a file is shown selected in its folder. Only where `FILE_MANAGER` is true.
pub fn show_in_file_manager(path: &str, is_dir: bool) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(if is_dir { path.to_string() } else { format!("/select,{path}") }).spawn();
    }
    #[cfg(not(windows))]
    let _ = (path, is_dir);
}

/// Open a web page (already checked by the caller) in the default browser. Windows: `ShellExecuteW` (no detour through
/// `explorer`). Elsewhere nothing happens yet.
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
    let _ = url;
}
