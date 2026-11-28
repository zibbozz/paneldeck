//! Desktop-Benachrichtigungen (Windows-Toast, Linux: Freedesktop-Benachrichtigungen über D-Bus, macOS: Mitteilungen über
//! `osascript`, #150).

/// Native file and folder dialogs exist (Windows: `rfd`; Linux: `rfd` over the XDG desktop portal, #149; macOS: `rfd` with NSOpenPanel/NSSavePanel, #150). Without them the
/// dialog functions return `None` at once, and callers that save something fall back to a fixed folder.
pub const DIALOGS: bool = cfg!(any(windows, unix));

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

/// Linux (#149): `org.freedesktop.Notifications.Notify` über D-Bus (zbus, im Hintergrund). Ohne Benachrichtigungsdienst
/// passiert nichts. `sound`: Hinweis `sound-name` (Freedesktop-Klang „message-new-instant“), sonst `suppress-sound`.
#[cfg(all(unix, not(target_os = "macos")))]
pub fn toast(title: &str, body: &str, sound: bool) {
    let (title, body) = (title.to_string(), body.to_string());
    std::thread::spawn(move || {
        let _ = linux_notify(&title, &body, sound);
    });
}

#[cfg(all(unix, not(target_os = "macos")))]
fn linux_notify(title: &str, body: &str, sound: bool) -> zbus::Result<()> {
    use std::collections::HashMap;
    use zbus::zvariant::Value;
    let conn = zbus::blocking::Connection::session()?;
    let mut hints: HashMap<&str, Value> = HashMap::new();
    hints.insert("desktop-entry", Value::from("paneldeck"));
    if sound {
        hints.insert("sound-name", Value::from("message-new-instant"));
    } else {
        hints.insert("suppress-sound", Value::from(true));
    }
    let actions: Vec<&str> = Vec::new();
    // Notify(app_name, replaces_id, app_icon, summary, body, actions, hints, expire_timeout)
    conn.call_method(
        Some("org.freedesktop.Notifications"),
        "/org/freedesktop/Notifications",
        Some("org.freedesktop.Notifications"),
        "Notify",
        &("Paneldeck", 0u32, "paneldeck", title, body, actions, hints, -1i32),
    )?;
    Ok(())
}

/// macOS (#150): `display notification` über `osascript` im Hintergrund. Die unsignierte App kann
/// `UNUserNotificationCenter` nicht verlässlich nutzen; die Mitteilung erscheint deshalb unter „Skript-Editor“ (in den
/// Systemeinstellungen → Mitteilungen erlaubt bzw. abschaltbar). Titel und Text gehen als Argumente an das Skript (`on run
/// argv`), nie in den Skripttext – kein Quoting nötig.
#[cfg(target_os = "macos")]
pub fn toast(title: &str, body: &str, sound: bool) {
    let args = osascript_args(title, body, sound);
    std::thread::spawn(move || {
        let _ = std::process::Command::new("/usr/bin/osascript")
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    });
}

/// Argumente für `osascript`: das Skript (fest), dann ein festes erstes Argument `paneldeck` und erst danach Titel und
/// Text als `argv` (Steuerzeichen entfernt, gekürzt). Das feste Argument beendet die Optionen von `osascript` (getopt
/// hört beim ersten Nicht-Options-Argument auf): Ein Titel wie `-e …` oder `-l JavaScript` (Terminaltitel per OSC 0/2
/// sind fremder Text) wird so nie als Option gelesen.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn osascript_args(title: &str, body: &str, sound: bool) -> Vec<String> {
    let clean = |s: &str, max: usize| s.chars().filter(|c| !c.is_control() || *c == '\n').take(max).collect::<String>();
    let show = if sound {
        "display notification (item 3 of argv) with title (item 2 of argv) sound name \"default\""
    } else {
        "display notification (item 3 of argv) with title (item 2 of argv)"
    };
    vec!["-e".into(), "on run argv".into(), "-e".into(), show.into(), "-e".into(), "end run".into(), "paneldeck".into(), clean(title, 200), clean(body, 1000)]
}

#[cfg(test)]
mod tests {
    #[test]
    fn osascript_gets_text_as_arguments_only() {
        let a = super::osascript_args("Build \"done\"", "line1\nline2\u{7}", true);
        assert_eq!(a.len(), 9);
        assert_eq!(&a[6], "paneldeck", "fixed first argument ends the options of osascript");
        assert_eq!(&a[7], "Build \"done\"", "title is an argument, never part of the script");
        assert_eq!(&a[8], "line1\nline2", "control characters (bell) removed");
        assert!(a[3].contains("item 3 of argv") && a[3].contains("title (item 2 of argv)") && a[3].contains("sound name"));
        assert!(!super::osascript_args("t", "b", false)[3].contains("sound"));
        assert!(a[..6].iter().all(|s| !s.contains("done")));
        // titles that look like options (e.g. a terminal title set by a program) stay arguments after the fixed one
        for t in ["-e x", "-l JavaScript", "-s", "-"] {
            let a = super::osascript_args(t, "b", false);
            assert_eq!((a[6].as_str(), a[7].as_str()), ("paneldeck", t));
            assert_eq!(a[..6], super::osascript_args("plain", "b", false)[..6], "script unchanged");
        }
    }
}

/// Öffnen-Dialog für beliebige Dateien (Editor, #54); `start` = Startordner, wenn er lokal existiert.
#[cfg(any(windows, unix))]
pub fn pick_file(start: &str) -> Option<std::path::PathBuf> {
    let mut d = rfd::FileDialog::new();
    if !crate::pathfmt::is_remote(start) && !crate::wsl::is_wsl_unc(start) && std::path::Path::new(start).is_dir() {
        d = d.set_directory(start);
    }
    d.pick_file()
}

#[cfg(not(any(windows, unix)))]
pub fn pick_file(_start: &str) -> Option<std::path::PathBuf> {
    None
}

/// Klangdatei wählen (Reiter „Zeit“, #164): WAV oder MP3.
#[cfg(any(windows, unix))]
pub fn pick_sound() -> Option<std::path::PathBuf> {
    rfd::FileDialog::new().add_filter("WAV/MP3", &["wav", "mp3"]).pick_file()
}

#[cfg(not(any(windows, unix)))]
pub fn pick_sound() -> Option<std::path::PathBuf> {
    None
}

/// Öffnen-Dialog für Bilder (Büroklammer im Chat, #357; mehrere möglich); `start` wie bei [`pick_file`].
#[cfg(any(windows, unix))]
pub fn pick_images(start: &str) -> Vec<std::path::PathBuf> {
    let mut d = rfd::FileDialog::new().add_filter(crate::state::t("Bilder", "Images"), crate::chat::images::EXTENSIONS);
    if !crate::pathfmt::is_remote(start) && !crate::wsl::is_wsl_unc(start) && std::path::Path::new(start).is_dir() {
        d = d.set_directory(start);
    }
    d.pick_files().unwrap_or_default()
}

#[cfg(not(any(windows, unix)))]
pub fn pick_images(_start: &str) -> Vec<std::path::PathBuf> {
    Vec::new()
}

/// „Speichern unter“ für beliebige Dateien (Editor, #54): Startordner `dir` (wenn lokal vorhanden), Vorschlag `name`.
#[cfg(any(windows, unix))]
pub fn save_any(dir: Option<&std::path::Path>, name: &str) -> Option<std::path::PathBuf> {
    let mut d = rfd::FileDialog::new().set_file_name(name);
    if let Some(dir) = dir.filter(|p| !crate::pathfmt::is_remote(&p.to_string_lossy()) && p.is_dir()) {
        d = d.set_directory(dir);
    }
    d.save_file()
}

#[cfg(not(any(windows, unix)))]
pub fn save_any(_dir: Option<&std::path::Path>, _name: &str) -> Option<std::path::PathBuf> {
    None
}

/// Speichern-Dialog (Windows); anderswo `None`.
#[cfg(any(windows, unix))]
pub fn save_file(default_name: &str) -> Option<std::path::PathBuf> {
    rfd::FileDialog::new().set_file_name(default_name).add_filter("Text", &["txt"]).save_file()
}

#[cfg(not(any(windows, unix)))]
pub fn save_file(_default_name: &str) -> Option<std::path::PathBuf> {
    None
}

/// Speichern-Dialog für JSON-Dateien (Vorlagen); anderswo `None`.
#[cfg(any(windows, unix))]
pub fn save_json(default_name: &str) -> Option<std::path::PathBuf> {
    rfd::FileDialog::new().set_file_name(default_name).add_filter("JSON", &["json"]).save_file()
}

#[cfg(not(any(windows, unix)))]
pub fn save_json(_default_name: &str) -> Option<std::path::PathBuf> {
    None
}

/// Öffnen-Dialog für ein Hintergrundbild eines Themes (#387); anderswo `None`.
#[cfg(any(windows, unix))]
pub fn pick_background_image() -> Option<std::path::PathBuf> {
    rfd::FileDialog::new().add_filter(crate::state::t("Bilder", "Images"), crate::theme::background::IMAGE_EXTENSIONS).pick_file()
}

#[cfg(not(any(windows, unix)))]
pub fn pick_background_image() -> Option<std::path::PathBuf> {
    None
}

/// Öffnen-Dialog für den Import eines Themes: `.json` oder `.zip` mit Hintergrundbild (#387); anderswo `None`.
#[cfg(any(windows, unix))]
pub fn open_theme() -> Option<std::path::PathBuf> {
    rfd::FileDialog::new().add_filter("Theme", &["json", "zip"]).pick_file()
}

#[cfg(not(any(windows, unix)))]
pub fn open_theme() -> Option<std::path::PathBuf> {
    None
}

/// Speichern-Dialog für ZIP-Dateien (Theme mit Hintergrundbild, #387); anderswo `None`.
#[cfg(any(windows, unix))]
pub fn save_zip(default_name: &str) -> Option<std::path::PathBuf> {
    rfd::FileDialog::new().set_file_name(default_name).add_filter("ZIP", &["zip"]).save_file()
}

#[cfg(not(any(windows, unix)))]
pub fn save_zip(_default_name: &str) -> Option<std::path::PathBuf> {
    None
}

/// Öffnen-Dialog für JSON-Dateien (Vorlagen); anderswo `None`.
#[cfg(any(windows, unix))]
pub fn open_json() -> Option<std::path::PathBuf> {
    rfd::FileDialog::new().add_filter("JSON", &["json"]).pick_file()
}

#[cfg(not(any(windows, unix)))]
pub fn open_json() -> Option<std::path::PathBuf> {
    None
}

/// Speichern-Dialog für CSV-Dateien (Ticket-Export); anderswo `None`.
#[cfg(any(windows, unix))]
pub fn save_csv(default_name: &str) -> Option<std::path::PathBuf> {
    rfd::FileDialog::new().set_file_name(default_name).add_filter("CSV", &["csv"]).save_file()
}

#[cfg(not(any(windows, unix)))]
pub fn save_csv(_default_name: &str) -> Option<std::path::PathBuf> {
    None
}

/// Speichern-Dialog für Markdown-Dateien (Systemprompt); anderswo `None`.
#[cfg(any(windows, unix))]
pub fn save_md(default_name: &str) -> Option<std::path::PathBuf> {
    rfd::FileDialog::new().set_file_name(default_name).add_filter("Markdown", &["md"]).save_file()
}

#[cfg(not(any(windows, unix)))]
pub fn save_md(_default_name: &str) -> Option<std::path::PathBuf> {
    None
}

/// Öffnen-Dialog für CSV-Dateien (Ticket-Import); anderswo `None`.
#[cfg(any(windows, unix))]
pub fn open_csv() -> Option<std::path::PathBuf> {
    rfd::FileDialog::new().add_filter("CSV", &["csv"]).pick_file()
}

#[cfg(not(any(windows, unix)))]
pub fn open_csv() -> Option<std::path::PathBuf> {
    None
}

/// Öffnen-Dialog für iCal-Dateien (Kalender-Import, #164); anderswo `None`.
#[cfg(any(windows, unix))]
pub fn open_ics() -> Option<std::path::PathBuf> {
    rfd::FileDialog::new().add_filter("iCalendar", &["ics", "ical", "ifb", "icalendar"]).pick_file()
}

#[cfg(not(any(windows, unix)))]
pub fn open_ics() -> Option<std::path::PathBuf> {
    None
}

/// Speichern-Dialog für iCal-Dateien (Kalender-Export, #164); anderswo `None`.
#[cfg(any(windows, unix))]
pub fn save_ics(default_name: &str) -> Option<std::path::PathBuf> {
    rfd::FileDialog::new().set_file_name(default_name).add_filter("iCalendar", &["ics"]).save_file()
}

#[cfg(not(any(windows, unix)))]
pub fn save_ics(_default_name: &str) -> Option<std::path::PathBuf> {
    None
}

/// Ordner-Auswahldialog.
#[cfg(any(windows, unix))]
pub fn pick_folder(start: &str) -> Option<String> {
    let mut d = rfd::FileDialog::new();
    // Netzwerk- und WSL-Pfade nicht prüfen (UI-Thread: SMB-Verbindung bzw. Start der WSL-VM, #216).
    if !crate::pathfmt::is_remote(start) && !crate::wsl::is_wsl_unc(start) && std::path::Path::new(start).is_dir() {
        d = d.set_directory(start);
    }
    d.pick_folder().map(|p| p.to_string_lossy().to_string())
}

#[cfg(not(any(windows, unix)))]
pub fn pick_folder(_start: &str) -> Option<String> {
    None
}

/// Datei-Auswahldialog für JSON-Dateien (MCP-Konfiguration, #289). `start` ist ein Startordner: ein
/// `\\wsl.localhost\…`-Pfad wird ungeprüft gesetzt (Prüfen könnte die WSL-VM starten), ein lokaler nur, wenn er existiert.
#[cfg(any(windows, unix))]
pub fn pick_json(start: &str) -> Option<String> {
    let mut d = rfd::FileDialog::new().add_filter("JSON", &["json"]);
    if crate::wsl::is_wsl_unc(start) || (!start.is_empty() && crate::claude_opts::check_path(start, true).is_ok()) {
        d = d.set_directory(start);
    }
    d.pick_file().map(|p| p.to_string_lossy().to_string())
}

#[cfg(not(any(windows, unix)))]
pub fn pick_json(_start: &str) -> Option<String> {
    None
}

/// Ordner-Auswahldialog mit Startordner ohne vorherige Prüfung (für `\\wsl.localhost\…`: schon das Prüfen kann die
/// WSL-VM starten und blockieren; fehlt der Ordner, zeigt der Dialog einen Standardordner).
#[cfg(any(windows, unix))]
pub fn pick_folder_unchecked(start: &str) -> Option<String> {
    rfd::FileDialog::new().set_directory(start).pick_folder().map(|p| p.to_string_lossy().to_string())
}

#[cfg(not(any(windows, unix)))]
pub fn pick_folder_unchecked(_start: &str) -> Option<String> {
    None
}
