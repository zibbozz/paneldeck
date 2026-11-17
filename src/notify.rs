//! Desktop-Benachrichtigungen (Windows-Toast). Auf anderen Systemen ohne Wirkung.

#[cfg(windows)]
pub fn toast(title: &str, body: &str, sound: bool) {
    use tauri_winrt_notification::{Duration, Sound, Toast};
    let (title, body) = (title.to_string(), body.to_string());
    std::thread::spawn(move || {
        let mut t = Toast::new(Toast::POWERSHELL_APP_ID)
            .title(&title)
            .text1(&body)
            .duration(Duration::Short);
        if sound {
            t = t.sound(Some(Sound::Default));
        }
        let _ = t.show();
    });
}

#[cfg(not(windows))]
pub fn toast(_title: &str, _body: &str, _sound: bool) {}

/// Speichern-Dialog (Windows); anderswo `None`.
#[cfg(windows)]
pub fn save_file(default_name: &str) -> Option<std::path::PathBuf> {
    rfd::FileDialog::new().set_file_name(default_name).add_filter("Text", &["txt"]).save_file()
}

#[cfg(not(windows))]
pub fn save_file(_default_name: &str) -> Option<std::path::PathBuf> {
    None
}

/// Speichern-Dialog für JSON-Dateien (Vorlagen); anderswo `None`.
#[cfg(windows)]
pub fn save_json(default_name: &str) -> Option<std::path::PathBuf> {
    rfd::FileDialog::new().set_file_name(default_name).add_filter("JSON", &["json"]).save_file()
}

#[cfg(not(windows))]
pub fn save_json(_default_name: &str) -> Option<std::path::PathBuf> {
    None
}

/// Öffnen-Dialog für JSON-Dateien (Vorlagen); anderswo `None`.
#[cfg(windows)]
pub fn open_json() -> Option<std::path::PathBuf> {
    rfd::FileDialog::new().add_filter("JSON", &["json"]).pick_file()
}

#[cfg(not(windows))]
pub fn open_json() -> Option<std::path::PathBuf> {
    None
}

/// Speichern-Dialog für CSV-Dateien (Ticket-Export); anderswo `None`.
#[cfg(windows)]
pub fn save_csv(default_name: &str) -> Option<std::path::PathBuf> {
    rfd::FileDialog::new().set_file_name(default_name).add_filter("CSV", &["csv"]).save_file()
}

#[cfg(not(windows))]
pub fn save_csv(_default_name: &str) -> Option<std::path::PathBuf> {
    None
}

/// Speichern-Dialog für Markdown-Dateien (Systemprompt); anderswo `None`.
#[cfg(windows)]
pub fn save_md(default_name: &str) -> Option<std::path::PathBuf> {
    rfd::FileDialog::new().set_file_name(default_name).add_filter("Markdown", &["md"]).save_file()
}

#[cfg(not(windows))]
pub fn save_md(_default_name: &str) -> Option<std::path::PathBuf> {
    None
}

/// Öffnen-Dialog für CSV-Dateien (Ticket-Import); anderswo `None`.
#[cfg(windows)]
pub fn open_csv() -> Option<std::path::PathBuf> {
    rfd::FileDialog::new().add_filter("CSV", &["csv"]).pick_file()
}

#[cfg(not(windows))]
pub fn open_csv() -> Option<std::path::PathBuf> {
    None
}

/// Ordner-Auswahldialog.
#[cfg(windows)]
pub fn pick_folder(start: &str) -> Option<String> {
    let mut d = rfd::FileDialog::new();
    // Netzwerk- und WSL-Pfade nicht prüfen (UI-Thread: SMB-Verbindung bzw. Start der WSL-VM, #216).
    if !crate::pathfmt::is_remote_unc(start) && !crate::wsl::is_wsl_unc(start) && std::path::Path::new(start).is_dir() {
        d = d.set_directory(start);
    }
    d.pick_folder().map(|p| p.to_string_lossy().to_string())
}

#[cfg(not(windows))]
pub fn pick_folder(_start: &str) -> Option<String> {
    None
}

/// Datei-Auswahldialog (z. B. MCP-Konfiguration, #182); startet im Ordner von `start`, wenn das ein lokaler Pfad ist
/// (Netzwerkpfade werden nicht berührt).
#[cfg(windows)]
pub fn pick_file(start: &str) -> Option<String> {
    let mut d = rfd::FileDialog::new();
    if let Some(dir) = std::path::Path::new(start).parent().filter(|p| crate::claude_opts::check_path(&p.to_string_lossy(), true).is_ok()) {
        d = d.set_directory(dir);
    }
    d.pick_file().map(|p| p.to_string_lossy().to_string())
}

#[cfg(not(windows))]
pub fn pick_file(_start: &str) -> Option<String> {
    None
}

/// Ordner-Auswahldialog mit Startordner ohne vorherige Prüfung (für `\\wsl.localhost\…`: schon das Prüfen kann die
/// WSL-VM starten und blockieren; fehlt der Ordner, zeigt der Dialog einen Standardordner).
#[cfg(windows)]
pub fn pick_folder_unchecked(start: &str) -> Option<String> {
    rfd::FileDialog::new().set_directory(start).pick_folder().map(|p| p.to_string_lossy().to_string())
}

#[cfg(not(windows))]
pub fn pick_folder_unchecked(_start: &str) -> Option<String> {
    None
}
