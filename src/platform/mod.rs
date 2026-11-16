//! Platform layer: the parts of Paneldeck that depend on the operating system.
//!
//! Every module here has the same shape: a small, documented function surface that the rest of the program calls, a
//! Windows implementation (today's code, unchanged) and a stub for other targets (`cfg(not(windows))`). Stubs either do
//! nothing (where that is harmless, e.g. a notification or a window style) or report "not supported" (`unsupported()`),
//! so the crate compiles and the tests run on Linux and macOS. Real Linux/macOS implementations will replace the stubs
//! (#149, #150).
//!
//! Areas in this folder:
//! - `chrome`: borderless main window – rounded corners (DWM) and the Snap Layouts flyout at the maximize button.
//! - `clipboard`: read an image from the clipboard (text goes through egui).
//! - `console`: command-line output of the GUI executable (attach to the parent console, message box).
//! - `fonts`: system font files used as fallbacks (Segoe UI, CJK, symbols) and optional terminal fonts.
//! - `fs`: share modes and reparse points.
//! - `keyboard`: physical key state (image paste with Ctrl+V, Ctrl+Insert).
//! - `notify`: desktop notifications and native file/folder dialogs.
//! - `open`: open files/URLs with the default program, show items in the file manager.
//! - `process`: raw command line, process tree, "is this PID alive", waiting for a PID, hidden child processes,
//!   elevated launch, how `claude` is started, file sharing/reparse-point details.
//! - `registry`: per-user settings store of the system (HKCU) – autostart, Explorer context menu, install location.
//! - `secret`: encryption of API keys for the current user (DPAPI).
//! - `tray`: tray icon with menu.
//! - `window`: native window handle, hide to tray, find own windows, monitor work areas.
//!
//! OS-specific parts that stay next to their portable interface (they implement a trait of their domain module):
//! - HTTP: `app::update::http::Transport` (self-update, GET only) and `llm::http::Transport` (LLM servers); the Windows
//!   implementations are WinHTTP in `app/update/http/winhttp.rs` and `llm/http/winhttp.rs`. Other targets get a
//!   transport that answers "not supported".
//! - Shells: `shells::detect` lists the shells of the system (PowerShell, cmd, Git Bash, WSL on Windows; `$SHELL` elsewhere).
//! - Self-update install path: `app::update::install` (NSIS installer `/S /D=…`, swapping `paneldeck.exe`); it uses
//!   `registry` and `process` from here and is only offered where `app::update::install::detect` finds a flavor.
//!
//! Already portable (no platform code needed): terminals (`portable-pty`: ConPTY on Windows, openpty elsewhere),
//! terminal emulation (`alacritty_terminal`), the single-instance lock (`File::try_lock`), the IPC between bridge and
//! hub (TCP on 127.0.0.1), the data folder search (`paths`).
//!
//! Test scripts (`tools/*.ps1`) and the installer (`installer/`) are Windows-only and not part of the crate.

pub mod chrome;
pub mod clipboard;
pub mod console;
pub mod fonts;
pub mod fs;
pub mod keyboard;
pub mod notify;
pub mod open;
pub mod process;
pub mod registry;
pub mod secret;
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
