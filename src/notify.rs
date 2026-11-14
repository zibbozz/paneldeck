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
