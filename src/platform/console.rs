//! Textausgabe der Kommandozeile (`--help`, `--version`, Fehler). Die Release-.exe ist eine GUI-Anwendung
//! (`windows_subsystem = "windows"`) und hat kein eigenes Konsolenfenster. Reihenfolge:
//! 1. umgeleitete Standardausgabe (Pipe/Datei, z. B. `paneldeck --help > hilfe.txt` oder `| more`) → UTF-8 dorthin;
//! 2. Konsole des aufrufenden Prozesses (`AttachConsole(ATTACH_PARENT_PROCESS)`) → `WriteConsoleW` (Umlaute korrekt);
//! 3. sonst (Start aus Explorer/Verknüpfung) ein Meldungsfenster, bei Fehlern mit Fehlersymbol.

/// Normale Ausgabe (stdout).
pub fn print(text: &str) {
    emit(text, false);
}

/// Fehlermeldung (stderr).
pub fn error(text: &str) {
    emit(text, true);
}

/// Nur stderr, nie stdout, Konsole oder Meldungsfenster (für `mcp`: stdout ist der Protokollkanal).
pub fn error_stderr(text: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "{}", text.trim_end());
}

#[cfg(not(windows))]
fn emit(text: &str, is_err: bool) {
    if is_err { eprintln!("{text}") } else { println!("{text}") }
}

#[cfg(windows)]
fn emit(text: &str, is_err: bool) {
    use windows_sys::Win32::System::Console::{AttachConsole, FreeConsole, GetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE};
    let text = if text.ends_with('\n') { text.to_string() } else { format!("{text}\n") };
    // 1. Umgeleitete oder geerbte Standard-Handles (bei Fehlern zuerst stderr, dann stdout).
    let ids: &[_] = if is_err { &[STD_ERROR_HANDLE, STD_OUTPUT_HANDLE] } else { &[STD_OUTPUT_HANDLE] };
    for &id in ids {
        // SAFETY: GetStdHandle hat keine Vorbedingungen.
        let h = unsafe { GetStdHandle(id) };
        if write_handle(h, &text) {
            return;
        }
    }
    // 2. Konsole des Elternprozesses (cmd, PowerShell). Die Shell wartet nicht auf eine GUI-Anwendung und hat ihre
    //    Eingabezeile evtl. schon geschrieben – deshalb mit einer Leerzeile beginnen.
    // SAFETY: AttachConsole/FreeConsole ohne Vorbedingungen; der Prozess hat vorher keine eigene Konsole.
    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } != 0 {
        let ok = std::fs::OpenOptions::new().write(true).open("CONOUT$").is_ok_and(|f| {
            use std::os::windows::io::AsRawHandle;
            write_handle(f.as_raw_handle() as _, &format!("\n{text}"))
        });
        unsafe { FreeConsole() };
        if ok {
            return;
        }
    }
    // 3. Meldungsfenster.
    message_box(&text, is_err);
}

/// In ein Handle schreiben: Konsole → `WriteConsoleW`, sonst UTF-8-Bytes. `false` bei ungültigem Handle/Fehler.
#[cfg(windows)]
fn write_handle(h: windows_sys::Win32::Foundation::HANDLE, text: &str) -> bool {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{GetConsoleMode, WriteConsoleW};
    if h.is_null() || h == INVALID_HANDLE_VALUE {
        return false;
    }
    let mut mode = 0u32;
    // SAFETY: h ist ein gültiges Handle (geprüft); mode ist ein gültiger Zeiger.
    if unsafe { GetConsoleMode(h, &mut mode) } != 0 {
        let w: Vec<u16> = text.encode_utf16().collect();
        let mut done = 0u32;
        let mut off = 0usize;
        while off < w.len() {
            let n = (w.len() - off).min(8192);
            // SAFETY: Puffer und Länge passen zusammen.
            if unsafe { WriteConsoleW(h, w[off..].as_ptr().cast(), n as u32, &mut done, std::ptr::null()) } == 0 || done == 0 {
                return false;
            }
            off += done as usize;
        }
        return true;
    }
    use std::io::Write;
    use std::os::windows::io::FromRawHandle;
    // SAFETY: Handle gehört dem Prozess (Standard-Handle bzw. geöffnete Datei); ManuallyDrop verhindert das Schließen.
    let mut f = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_handle(h as _) });
    f.write_all(text.as_bytes()).is_ok() && f.flush().is_ok()
}

#[cfg(windows)]
fn message_box(text: &str, is_err: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK};
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (body, title) = (wide(text.trim_end()), wide("Paneldeck"));
    let icon = if is_err { MB_ICONERROR } else { MB_ICONINFORMATION };
    // SAFETY: nullterminierte UTF-16-Puffer, kein Elternfenster.
    unsafe { MessageBoxW(std::ptr::null_mut(), body.as_ptr(), title.as_ptr(), MB_OK | icon) };
}
