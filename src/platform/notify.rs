//! Desktop-Benachrichtigungen (Windows-Toast) und Datei-/Ordnerdialoge. Auf anderen Systemen ohne Wirkung bzw. `None`.

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

/// Native file dialogs exist (Windows: `rfd`). Without them every dialog function returns `None` and callers fall back
/// (e.g. save into `exports/` in the data folder).
pub const DIALOGS: bool = cfg!(windows);

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
    if std::path::Path::new(start).is_dir() {
        d = d.set_directory(start);
    }
    d.pick_folder().map(|p| p.to_string_lossy().to_string())
}

#[cfg(not(windows))]
pub fn pick_folder(_start: &str) -> Option<String> {
    None
}
