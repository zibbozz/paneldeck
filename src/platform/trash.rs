//! Dateien in den Papierkorb legen (`delete_file` der Chat-Werkzeuge, #338). Nie endgültig löschen: geht der
//! Papierkorb nicht, bleibt die Datei und es gibt einen Fehler.
//!
//! * Windows: `SHFileOperationW` mit `FOF_ALLOWUNDO`; müsste Windows endgültig löschen (zu groß, kein Papierkorb auf
//!   dem Laufwerk), fragt es wegen `FOF_WANTNUKEWARNING` zuerst den Nutzer. Netzwerkpfade werden abgelehnt.
//! * Linux: Papierkorb nach freedesktop.org (`$XDG_DATA_HOME/Trash` bzw. `~/.local/share/Trash`, `files/` + `info/`).
//! * macOS: `~/.Trash`.
//!
//! Unter Linux und macOS nur auf demselben Dateisystem (umbenennen); sonst Fehler statt Kopieren und Löschen.

use std::path::Path;

/// `p` in den Papierkorb legen. `Err` = Text für das Modell (englisch).
pub fn to_trash(p: &Path) -> Result<(), String> {
    imp::to_trash(p)
}

#[cfg(windows)]
mod imp {
    use std::path::Path;
    use windows_sys::Win32::UI::Shell::{SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FOF_WANTNUKEWARNING, FO_DELETE, SHFILEOPSTRUCTW};

    pub fn to_trash(p: &Path) -> Result<(), String> {
        let s = p.display().to_string();
        // `\\?\C:\…` versteht die Shell nicht: ohne Vorsatz; Netzwerk- und Gerätepfade haben keinen Papierkorb.
        let s = s.strip_prefix(r"\\?\").unwrap_or(&s).to_string();
        if s.starts_with(r"\\") || !s.as_bytes().get(1).is_some_and(|b| *b == b':') {
            return Err("this path has no recycle bin (network or device path)".into());
        }
        let wide: Vec<u16> = s.encode_utf16().chain([0, 0]).collect();
        // SAFETY: alle Felder gültig belegt (Nullzeiger, wo erlaubt), `pFrom` doppelt nullterminiert und lebt bis zum Ende.
        let mut op: SHFILEOPSTRUCTW = unsafe { std::mem::zeroed() };
        op.wFunc = FO_DELETE;
        op.pFrom = wide.as_ptr();
        op.fFlags = (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT | FOF_WANTNUKEWARNING) as u16;
        let r = unsafe { SHFileOperationW(&mut op) };
        if r != 0 {
            return Err(format!("the recycle bin refused it (error 0x{r:x}; in use or protected?)"));
        }
        if op.fAnyOperationsAborted != 0 {
            return Err("the user cancelled deleting it".into());
        }
        if std::fs::symlink_metadata(p).is_ok() {
            return Err("it still exists after moving it to the recycle bin".into());
        }
        Ok(())
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use std::path::{Path, PathBuf};

    fn trash_dir() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
        Some(base.join("Trash"))
    }

    /// Prozent-Kodierung für `Path=` in der `.trashinfo`.
    fn encode(p: &str) -> String {
        let mut o = String::new();
        for b in p.bytes() {
            if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
                o.push(b as char);
            } else {
                o.push_str(&format!("%{b:02X}"));
            }
        }
        o
    }

    pub fn to_trash(p: &Path) -> Result<(), String> {
        let dir = trash_dir().ok_or("no home folder for the recycle bin")?;
        let (files, info) = (dir.join("files"), dir.join("info"));
        std::fs::create_dir_all(&files).and_then(|_| std::fs::create_dir_all(&info)).map_err(|e| format!("the recycle bin is not available ({e})"))?;
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or("no file name")?;
        let mut target = name.clone();
        let mut n = 1;
        let info_file = loop {
            let i = info.join(format!("{target}.trashinfo"));
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&i) {
                Ok(mut f) => {
                    use std::io::Write;
                    let now = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S");
                    let _ = write!(f, "[Trash Info]\nPath={}\nDeletionDate={now}\n", encode(&p.to_string_lossy()));
                    break i;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && n < 1000 => {
                    n += 1;
                    target = format!("{name}.{n}");
                }
                Err(e) => return Err(format!("the recycle bin is not available ({e})")),
            }
        };
        if let Err(e) = std::fs::rename(p, files.join(&target)) {
            let _ = std::fs::remove_file(&info_file);
            return Err(if e.raw_os_error() == Some(18) { "the recycle bin is on another file system (nothing was deleted)".into() } else { format!("moving it to the recycle bin failed ({e})") });
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::path::{Path, PathBuf};

    pub fn to_trash(p: &Path) -> Result<(), String> {
        let home = std::env::var_os("HOME").map(PathBuf::from).ok_or("no home folder for the trash")?;
        let dir = home.join(".Trash");
        std::fs::create_dir_all(&dir).map_err(|e| format!("the trash is not available ({e})"))?;
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or("no file name")?;
        let mut target = dir.join(&name);
        let mut n = 1;
        while std::fs::symlink_metadata(&target).is_ok() && n < 1000 {
            n += 1;
            target = dir.join(format!("{name} {n}"));
        }
        std::fs::rename(p, &target).map_err(|e| if e.raw_os_error() == Some(18) { "the trash is on another volume (nothing was deleted)".to_string() } else { format!("moving it to the trash failed ({e})") })
    }
}

#[cfg(test)]
mod tests {
    // Bewusst kein Test, der wirklich in den Papierkorb des Rechners legt (er bliebe dort liegen); geprüft wird nur
    // die Ablehnung von Pfaden ohne Papierkorb.
    #[cfg(windows)]
    #[test]
    fn network_paths_are_refused() {
        assert!(super::to_trash(std::path::Path::new(r"\\server\share\x.txt")).is_err());
        assert!(super::to_trash(std::path::Path::new(r"\\?\UNC\server\share\x.txt")).is_err());
    }
}
