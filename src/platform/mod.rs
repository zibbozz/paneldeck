//! Platform layer (#148): the parts of Paneldeck that depend on the operating system.
//!
//! Every module here has the same shape: a small, documented function surface that the rest of the program calls, a
//! Windows implementation (the code as it was before the move, unchanged) and a stub or minimal implementation for other
//! targets (`cfg(not(windows))`). Stubs either do nothing (where that is harmless, e.g. a notification or a window
//! style) or report "not supported" (`unsupported()`), so the crate compiles and the tests run on Linux and macOS. Real
//! Linux/macOS implementations will replace the stubs (#149, #150).
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
//! - `secret`: encryption of API keys for the current user (DPAPI).
//! - `taskbar`: progress and attention state on the taskbar button (`ITaskbarList3`).
//! - `tray`: tray icon with menu.
//! - `window`: native window handle, hide to tray, find own windows, monitor work areas.
//!
//! OS-specific parts that stay next to their portable interface (they implement a trait or a narrow function of their
//! domain module):
//! - HTTP: `app::update::http::Transport` (self-update, GET only) and `llm::http::Transport` (LLM servers); the Windows
//!   implementations are WinHTTP in `app/update/http/winhttp.rs` and `llm/http/winhttp.rs`. Other targets get a
//!   transport that answers "not supported" (#149 decided on ureq + rustls there, with the same host rules).
//! - Shells: `shells::detect` lists the shells of the system (PowerShell, cmd, Git Bash, WSL on Windows; `$SHELL`
//!   elsewhere). `wsl` and `conpty` (bundled ConPTY) are Windows-only by nature and answer "none" elsewhere.
//! - Self-update install path: `app::update::install` (NSIS installer `/S /D=…`, swapping `paneldeck.exe`); it uses
//!   `registry` and `process` from here and is only offered where `SELF_UPDATE` is true.
//!
//! Already portable (no platform code needed): terminals (`portable-pty`: ConPTY on Windows, openpty elsewhere),
//! terminal emulation (`alacritty_terminal`), the single-instance lock (`File::try_lock`), the IPC between bridge and
//! hub (TCP on 127.0.0.1).
//!
//! Not decided here: the data folder search (`paths`) works everywhere, but off Windows its last resort is
//! `$HOME/Paneldeck` (the `%APPDATA%` slot falls back to `HOME`). XDG (`~/.local/share`) and
//! `~/Library/Application Support` follow in #149/#150.
//!
//! BLOCKER for #149/#150 (security): the MCP rules about the agent chain (#152) need the caller of a TCP connection.
//! `process::resolve_peer` finds it on Windows (TCP table, process tree, Job Object; `None` = refused). Elsewhere it
//! returns an empty chain: the caller is unknown, so only `PANELDECK_SESSION`/`PANELDECK_SESSION_KEY` tie a bridge to
//! its session, and Job Objects (`process::SessionJob`) do not exist. Before releasing on a new target, implement
//! `resolve_peer` there (Linux `/proc/net/tcp`, macOS libproc) with "unresolvable → refused" (#149 decision) and keep
//! its tests running in CI.
//!
//! Test scripts (`tools/*.ps1`) and the installer (`installer/`) are Windows-only and not part of the crate.

pub mod chrome;
pub mod clipboard;
pub mod console;
pub mod conpty;
pub mod fonts;
pub mod fs;
pub mod keyboard;
pub mod notify;
pub mod open;
pub mod process;
pub mod registry;
pub mod secret;
pub mod taskbar;
pub mod tray;
pub mod window;

/// Self-update (`app::update`): the release assets (NSIS installer, portable ZIP with `paneldeck.exe`) exist only for
/// Windows. Elsewhere `app::update::install::detect` finds no install flavor: a newer version is only announced.
pub const SELF_UPDATE: bool = cfg!(windows);

/// Error text for a function that has no implementation on this platform yet.
#[cfg_attr(windows, allow(dead_code))]
pub fn unsupported() -> String {
    crate::state::t("Auf diesem System noch nicht unterstützt", "Not supported on this platform yet").to_string()
}
