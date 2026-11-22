//! Platform layer (#148): the parts of Paneldeck that depend on the operating system.
//!
//! Every module here has the same shape: a small, documented function surface that the rest of the program calls, a
//! Windows implementation (the code as it was before the move, unchanged) and a stub or minimal implementation for other
//! targets (`cfg(not(windows))`). Stubs either do nothing (where that is harmless, e.g. a notification or a window
//! style) or report "not supported" (`unsupported()`), so the crate compiles and the tests run on Linux and macOS.
//! Linux implementations (#149, 2.5.0): `unix_proc` (caller attribution, session tags), `tls` (rustls with the system
//! trust store), `desktop` (XDG autostart, file-manager integration), `secret` (Secret Service or master password),
//! `tray` (StatusNotifierItem), `notify` (D-Bus notifications), `fonts` (fontconfig), `clipboard`, `open` (xdg-open).
//! macOS (#150, experimental, 2.5.0) shares `unix_proc`, `tls` and `secret` (key in the Keychain, else the master
//! password) and has its own `desktop` (LaunchAgent autostart, `desktop_macos.rs`), `tray` (menu-bar icon, tray-icon),
//! `notify` (`osascript`, dialogs through rfd), `fonts` (font folders read with ttf-parser), `open` (`open`).
//!
//! Areas in this folder:
//! - `chrome`: borderless main window – rounded corners (DWM) and the Snap Layouts flyout at the maximize button.
//! - `clipboard`: read an image from the clipboard (text goes through egui).
//! - `conpty`: console host of the terminals (bundled ConPTY next to the exe or the system one) and the hardened DLL
//!   search; elsewhere the PTY of `portable-pty` (openpty).
//! - `console`: command-line output of the GUI executable (attach to the parent console, message box).
//! - `fonts`: system font folder and the installed font families (GDI enumeration on Windows).
//! - `fs`: share modes, reparse points, drive letters and drive types (network drives).
//! - `keyboard`: physical key state (image paste with Ctrl+V, Ctrl+Insert) and the character of a key in the current
//!   keyboard layout.
//! - `notify`: desktop notifications and native file/folder dialogs (`DIALOGS`).
//! - `open`: open files/URLs with the default program, show items in the file manager.
//! - `process`: raw command line, peer of a local TCP connection, process tree, process names/windows, the Job Objects
//!   of the sessions (#168), "is this PID alive", waiting for a PID, hidden child processes, elevated launch, how
//!   `claude` is started.
//! - `registry`: per-user settings store of the system (HKCU) – autostart, Explorer context menu, install location.
//! - `secret`: encryption of API keys for the current user (DPAPI; Linux: Secret Service or master password).
//! - `sound`: play the sounds of the Time tab (#164; Windows: rodio, Linux/macOS: the system's players).
//! - `taskbar`: progress and attention state on the taskbar button (`ITaskbarList3`).
//! - `tray`: tray icon with menu.
//! - `window`: native window handle, hide to tray, find own windows, monitor work areas.
//!
//! OS-specific parts that stay next to their portable interface (they implement a trait or a narrow function of their
//! domain module):
//! - HTTP: `app::update::http::Transport` (self-update, GET only) and `llm::http::Transport` (LLM servers); the Windows
//!   implementations are WinHTTP in `app/update/http/winhttp.rs` and `llm/http/winhttp.rs`; Linux/macOS use ureq +
//!   rustls (`app/update/http/unix.rs`, `llm/http/unix.rs`, `tls`) with the same host rules.
//! - Shells: `shells::detect` lists the shells of the system (PowerShell, cmd, Git Bash, WSL on Windows; `/etc/shells`
//!   with `$SHELL` first, as login shells, elsewhere). `wsl` and `conpty` (bundled ConPTY) are Windows-only by nature and answer "none" elsewhere.
//! - Self-update install path: `app::update::install` (NSIS installer `/S /D=…`, swapping `paneldeck.exe`; Linux: swapping
//!   the `.AppImage`, #149); it uses `registry` and `process` from here and is only offered where `SELF_UPDATE` is true
//!   (Linux: only when running as an AppImage).
//!
//! Already portable (no platform code needed): terminals (`portable-pty`: ConPTY on Windows, openpty elsewhere),
//! terminal emulation (`alacritty_terminal`), the single-instance lock (`File::try_lock`), the IPC between bridge and
//! hub (TCP on 127.0.0.1).
//!
//! Data folder (`paths`): the `%APPDATA%` slot is `$XDG_DATA_HOME` / `~/.local/share` on Linux and
//! `~/Library/Application Support` on macOS; the portable folder only counts there if it already exists.
//!
//! Caller attribution (security, #152): the MCP rules about the agent chain need the caller of a TCP connection.
//! `process::resolve_peer` finds it on Windows (TCP table, process tree, Job Object) and on Linux/macOS
//! (`unix_proc`: `/proc/net/tcp` + `/proc/<pid>/fd` resp. libproc, process tree, a session tag in the environment
//! instead of Job Objects). Unresolvable → `None` = refused on every platform ("fail closed", #149 decision); the
//! tests run in the Linux and macOS CI jobs.
//!
//! Test scripts (`tools/*.ps1`) and the installer (`installer/`) are Windows-only and not part of the crate.

pub mod chrome;
pub mod clipboard;
pub mod console;
pub mod conpty;
#[cfg(all(unix, not(target_os = "macos")))]
pub mod desktop;
/// macOS (#150): LaunchAgent for the autostart (same function surface as the Linux `desktop`). Compiled for the tests
/// on every platform (pure functions).
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[path = "desktop_macos.rs"]
pub mod desktop_macos;
#[cfg(target_os = "macos")]
pub use desktop_macos as desktop;
pub mod fonts;
pub mod fs;
pub mod keyboard;
pub mod notify;
pub mod open;
pub mod process;
pub mod registry;
pub mod secret;
pub mod sound;
pub mod taskbar;
#[cfg(unix)]
pub mod tls;
pub mod trash;
pub mod tray;
#[cfg(unix)]
pub mod unix_proc;
pub mod window;

/// Self-update (`app::update`): the release assets (NSIS installer, portable ZIP with `paneldeck.exe`) exist only for
/// Windows. Elsewhere `app::update::install::detect` finds no install flavor: a newer version is only announced.
#[cfg_attr(unix, allow(dead_code))] // Linux: `app::update::install::appimage_flavor`, macOS: `macapp_flavor`
pub const SELF_UPDATE: bool = cfg!(windows);

/// Error text for a function that has no implementation on this platform yet.
#[allow(dead_code)]
pub fn unsupported() -> String {
    crate::state::t("Auf diesem System noch nicht unterstützt", "Not supported on this platform yet").to_string()
}
