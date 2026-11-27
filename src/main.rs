#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod afford;
mod app;
mod backup;
mod boxdraw;
mod calc;
mod calendar;
mod chat;
mod cleanup;
mod claude_opts;
mod db;
mod due;
mod editor;
mod clip;
mod conhost_wrap;
mod emoji;
mod pathfmt;
mod quote;
mod gitinfo;
mod glyphs;
mod gitops;
mod history;
mod hyperlink;
mod hooks;
mod cli;
mod help;
mod integration;
mod launch;
mod keys;
mod osc;
mod palette;
mod paths;
mod planning;
mod platform;
mod procs;
// Dienste des Editor-Reiters (#54): MCP-Werkzeuge, Suche in Dateien, Plattenänderungen, Einstellungen.
mod editor_svc;
mod prompts;
mod query;
mod search;
mod session_env;
mod layout;
mod links;
mod llm;
mod markdown;
mod mcp;
mod shellinit;
mod shells;
mod state;
mod style;
mod sysfonts;
mod tasks;
mod templates;
mod terminal;
mod undo;
mod vtkeys;
mod termview;
mod theme;
// Reiter „Zeit“ (#164): Wecker, Timer, Stoppuhren, Weltuhren, Geburtstage, Benachrichtigungen, Klänge (ohne egui).
mod time;
mod usage;
mod vtnotify;
mod wsl;

use platform::console;

/// True when the process was started minimized or hidden (e.g. `Start-Process -WindowStyle Minimized`, a shortcut set to
/// "Minimized"): STARTUPINFO carries the show state, which winit ignores.
#[cfg(windows)]
fn started_minimized() -> bool {
    use windows_sys::Win32::System::Threading::{GetStartupInfoW, STARTF_USESHOWWINDOW, STARTUPINFOW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_HIDE, SW_MINIMIZE, SW_SHOWMINIMIZED, SW_SHOWMINNOACTIVE};
    // SAFETY: GetStartupInfoW fills the given struct and cannot fail.
    let si = unsafe {
        let mut si: STARTUPINFOW = std::mem::zeroed();
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        GetStartupInfoW(&mut si);
        si
    };
    si.dwFlags & STARTF_USESHOWWINDOW != 0 && [SW_HIDE, SW_MINIMIZE, SW_SHOWMINIMIZED, SW_SHOWMINNOACTIVE].contains(&(si.wShowWindow as i32))
}

#[cfg(not(windows))]
fn started_minimized() -> bool {
    false
}

fn app_icon() -> eframe::egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon64.png")).unwrap_or_default()
}

/// Schreibt Abstürze nach `paneldeck-data/crash.log` (die App hat kein Konsolenfenster).
fn install_panic_hook() {
    let path = state::data_dir().join("crash.log");
    std::panic::set_hook(Box::new(move |info| {
        use std::io::Write;
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let bt = std::backtrace::Backtrace::force_capture();
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f, "[unix {secs}] Version {} – {info}\n{bt}\n", env!("CARGO_PKG_VERSION"));
        }
    }));
}

/// Datei-Sperre auf den Datenordner: zwei gleichzeitig gestartete Instanzen (z. B. Explorer-Kontextmenü auf mehreren
/// Ordnern, bevor die erste ihren IPC-Server bereithat) würden sich sonst `state.json` gegenseitig überschreiben.
/// Die Sperre endet mit dem Prozess.
enum InstanceLock {
    /// Gehalten (auch: schon vorher von diesem Prozess).
    Acquired,
    /// Eine andere Instanz hält die Sperre.
    Busy,
    /// `instance.lock` ließ sich nicht öffnen oder sperren (Ordner nicht beschreibbar o. Ä.).
    Unavailable(String),
}

fn acquire_instance_lock() -> InstanceLock {
    static LOCK: std::sync::OnceLock<std::fs::File> = std::sync::OnceLock::new();
    if LOCK.get().is_some() {
        return InstanceLock::Acquired; // schon gehalten (z. B. nach `--reset-settings`)
    }
    let dir = state::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let f = match std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join("instance.lock")) {
        Ok(f) => f,
        Err(e) => return InstanceLock::Unavailable(e.to_string()),
    };
    match f.try_lock() {
        Ok(()) => {
            let _ = LOCK.set(f);
            InstanceLock::Acquired
        }
        Err(std::fs::TryLockError::WouldBlock) => InstanceLock::Busy,
        Err(std::fs::TryLockError::Error(e)) => InstanceLock::Unavailable(e.to_string()),
    }
}

/// Rohe Kommandozeile mit eigener Zerlegung (`cli::split_command_line`), damit `"C:\"` und Pfade mit
/// Leerzeichen nach einer Laufwerkswurzel richtig ankommen. Ohne rohe Kommandozeile (`platform::process`): `std::env::args`.
fn command_line_args() -> Vec<String> {
    if let Some(raw) = platform::process::raw_command_line() {
        let v = cli::split_command_line(&raw);
        if !v.is_empty() {
            return v;
        }
    }
    std::env::args().collect()
}

/// Sprache für Ausgaben vor dem Start der Oberfläche: `--lang`, sonst `settings.json` des Datenordners (ohne
/// etwas anzulegen), sonst Englisch.
fn early_lang(arg: Option<state::Lang>, cli: &cli::Cli) -> state::Lang {
    arg.or_else(|| paths::peek_dir(&paths::inputs(cli)).and_then(|d| state::lang_from_disk(&d))).unwrap_or(state::Lang::En)
}

fn main() -> eframe::Result<()> {
    // Zuerst: DLLs nur noch aus Programmordner und System32 laden, nie aus dem Arbeitsordner (#183).
    platform::conpty::harden_dll_search();
    // Hook-Aufrufe von Claude Code (`--hook`) MÜSSEN vor der Einzelinstanz-Weiterleitung behandelt werden:
    // sie laufen mit demselben Datenordner wie die laufende App und dürfen nicht als „Fenster zeigen“ enden.
    if cli::is_hook_invocation(&std::env::args().collect::<Vec<_>>()) {
        std::process::exit(hooks::run_hook_client());
    }
    // Kommandozeile lesen (nach `--hook`, das eigene Argumente von Claude Code mitbringt). Fehler: Exit-Code 2.
    let args = command_line_args();
    // `mcp`: stdout gehört dem Protokoll – Meldungen nur auf stderr, nie ein Meldungsfenster.
    let mcp_mode = args.get(1).map(|s| s.as_str()) == Some("mcp");
    let cli = match cli::parse(&args, &cli::abs_from_cwd, &cli::fs_kind) {
        Ok(c) => c,
        Err(e) => {
            let msg = e.message(early_lang(cli::scan_lang(&args), &cli::Cli::default()));
            if mcp_mode { console::error_stderr(&msg) } else { console::error(&msg) }
            std::process::exit(2);
        }
    };
    cli::set(cli);
    let cli = cli::get();
    // Vom Deinstallationsprogramm: die globalen Claude-Hooks wieder entfernen, sonst zeigen sie auf eine gelöschte .exe.
    if cli.uninstall_hooks {
        // Am geltenden Ort und – mit CLAUDE_CONFIG_DIR – auch in ~/.claude, wohin ältere Versionen immer schrieben.
        let ok = hooks::global_settings_candidates().iter().all(|p| !p.exists() || hooks::uninstall_global(p).is_ok());
        // Ebenso eine Registrierung als MCP-Server in Claude Code, falls sie auf diese .exe zeigt.
        mcp::unregister_if_ours();
        // Autostart-Werte `Paneldeck (…)` von Instanzen mit Profil/Datenordner (den Standardwert löscht der Installer).
        integration::remove_instance_autostarts();
        // Explorer-Eintrag „In Paneldeck-Editor öffnen“ (#54), wenn er auf diese .exe zeigt.
        integration::remove_edit_context_if_ours();
        std::process::exit(if ok { 0 } else { 1 });
    }
    // Sprache für Hilfe und Meldungen: --lang > settings.json des Datenordners > Englisch.
    let lang = early_lang(cli.lang, cli);
    state::set_lang(lang);
    if cli.help {
        if cli.mcp {
            console::error_stderr(&help::text(lang));
            std::process::exit(0);
        }
        console::print(&help::text(lang));
        std::process::exit(0);
    }
    if cli.version {
        console::print(&format!("Paneldeck {}", env!("CARGO_PKG_VERSION")));
        std::process::exit(0);
    }
    // MCP-Server für KI-Agenten (`paneldeck.exe mcp`, stdio): vor Instanzsperre und Weiterleitung, sonst würde
    // „mcp“ als gewöhnlicher Start ein Fenster öffnen.
    if cli.mcp {
        std::process::exit(mcp::run_bridge());
    }
    // Vom Selbst-Update neu gestartet: warten, bis die alte Instanz beendet ist (sonst übergäbe sich diese an sie und
    // beendete sich), und PANELDECK_UPDATE_WAIT_PID entfernen, damit Terminals sie nicht erben.
    app::update::wait_for_previous_instance();
    // Datenordner einmal auflösen; ein unbrauchbarer ausdrücklich gewählter Ordner ist ein Fehler (Exit-Code 1).
    let dir = match paths::init(cli) {
        Ok(d) => d.path.clone(),
        Err(e) => {
            console::error(&e.message(lang));
            std::process::exit(1);
        }
    };
    if let Some(b) = &cli.backup_dir {
        if !paths::usable(b) {
            console::error(&format!(
                "Paneldeck: {} {} ({}).",
                state::t("Der Sicherungsordner lässt sich nicht anlegen oder beschreiben:", "The backup folder cannot be created or written:"),
                b.display(),
                state::t("angegeben mit --backup-dir", "given with --backup-dir")
            ));
            std::process::exit(1);
        }
    }
    install_panic_hook();
    // "paneldeck.exe.old" (Update über eine laufende Instanz) wird erst nach dem ersten gezeichneten Frame gelöscht –
    // siehe `app::update::install::remove_old_exe`.
    let cmd = match (&cli.open, &cli.edit) {
        (Some(p), _) => hooks::IpcCmd::Open { path: p.clone(), claude: cli.claude, origin: None },
        (None, Some(f)) => hooks::IpcCmd::Edit { path: f.clone() },
        (None, None) => hooks::IpcCmd::Show,
    };
    // `--reset-settings` nur, wenn keine Instanz mit diesem Datenordner läuft (sie würde die Datei wieder schreiben).
    if cli.reset_settings {
        // Die laufende Instanz hält die Instanzsperre (kein Weiterleiten: das Fenster soll nicht aufspringen).
        match acquire_instance_lock() {
            InstanceLock::Acquired => {}
            InstanceLock::Busy => {
                console::error(&format!(
                    "Paneldeck: {} {}",
                    state::t("--reset-settings wurde nicht ausgeführt: Paneldeck läuft bereits mit dem Datenordner", "--reset-settings was not applied: Paneldeck is already running with the data folder"),
                    dir.display()
                ));
                std::process::exit(1);
            }
            InstanceLock::Unavailable(e) => {
                console::error(&format!(
                    "Paneldeck: {} {} ({e})",
                    state::t("--reset-settings wurde nicht ausgeführt: die Instanzsperre (instance.lock) lässt sich nicht öffnen in", "--reset-settings was not applied: the instance lock (instance.lock) cannot be opened in"),
                    dir.display()
                ));
                std::process::exit(1);
            }
        }
        match state::reset_settings(&dir) {
            Ok(Some(b)) => {
                let _ = state::SETTINGS_RESET.set(b);
            }
            Ok(None) => {}
            Err(e) => {
                console::error(&format!("Paneldeck: {}: {e}", state::t("Einstellungen konnten nicht zurückgesetzt werden", "Settings could not be reset")));
                std::process::exit(1);
            }
        }
    } else {
        // Läuft bereits eine Instanz mit demselben Datenordner, übergeben wir an sie und beenden uns (kein doppelter Zugriff auf dieselben Daten).
        if !cli.tray && hooks::forward_to_running(&dir, &cmd) {
            return Ok(());
        }
        if cli.tray && hooks::forward_to_running(&dir, &hooks::IpcCmd::Show) {
            return Ok(());
        }
    }
    if matches!(acquire_instance_lock(), InstanceLock::Busy) {
        // Die andere Instanz startet gerade noch: ihren IPC-Server abwarten und an sie übergeben, statt Daten doppelt zu öffnen.
        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(250));
            if hooks::forward_to_running(&dir, &cmd) {
                break;
            }
        }
        return Ok(());
    }
    // Started in the tray (autostart) or minimized: open the window without activating it, so it does not take the
    // keyboard focus from the program the user is working in (with `--tray` it goes to the tray after the first frame).
    let quiet_start = cli.tray || started_minimized();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_active(!quiet_start)
            .with_title("Paneldeck")
            .with_inner_size([1320.0, 820.0])
            .with_min_inner_size([640.0, 400.0])
            .with_transparent(true)
            .with_decorations(false)
            .with_icon(app_icon()),
        ..Default::default()
    };
    // Linux (#149): Hub als Subreaper – losgelöste Nachfahren der Sitzungen bleiben im Baum des Hubs
    // (Aufrufer-Zuordnung, `platform::unix_proc`). Nur im Hub, nicht in Brücke/Weiterleitungen (die enden oben).
    #[cfg(target_os = "linux")]
    platform::unix_proc::become_subreaper();
    eframe::run_native(
        "Paneldeck",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
