#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod backup;
mod boxdraw;
mod calc;
mod db;
mod clip;
mod emoji;
mod pathfmt;
mod quote;
mod gitinfo;
mod gitops;
mod history;
mod hooks;
mod cli;
mod console;
mod help;
mod integration;
mod keys;
mod notify;
mod osc;
mod paths;
mod planning;
mod prompts;
mod query;
mod search;
mod layout;
mod links;
mod markdown;
mod mcp;
mod shells;
mod state;
mod style;
mod tasks;
mod templates;
mod terminal;
mod undo;
mod tray;
mod termview;
mod theme;
mod usage;
mod winctl;

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
/// `Err(())` = eine andere Instanz hält die Sperre. Die Sperre endet mit dem Prozess.
fn acquire_instance_lock() -> Result<(), ()> {
    static LOCK: std::sync::OnceLock<std::fs::File> = std::sync::OnceLock::new();
    if LOCK.get().is_some() {
        return Ok(()); // schon gehalten (z. B. nach `--reset-settings`)
    }
    let dir = state::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let Ok(f) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join("instance.lock")) else {
        return Ok(()); // Ordner nicht beschreibbar: nicht aussperren
    };
    match f.try_lock() {
        Ok(()) => {
            let _ = LOCK.set(f);
            Ok(())
        }
        Err(std::fs::TryLockError::WouldBlock) => Err(()),
        Err(_) => Ok(()),
    }
}

/// Sprache für Ausgaben vor dem Start der Oberfläche: `--lang`, sonst `settings.json` des Datenordners (ohne
/// etwas anzulegen), sonst Englisch.
fn early_lang(arg: Option<state::Lang>, cli: &cli::Cli) -> state::Lang {
    arg.or_else(|| paths::peek_dir(&paths::inputs(cli)).and_then(|d| state::lang_from_disk(&d))).unwrap_or(state::Lang::En)
}

fn main() -> eframe::Result<()> {
    // Hook-Aufrufe von Claude Code (`--hook`) MÜSSEN vor der Einzelinstanz-Weiterleitung behandelt werden:
    // sie laufen mit demselben Datenordner wie die laufende App und dürfen nicht als „Fenster zeigen“ enden.
    if std::env::args().any(|a| a == "--hook") {
        std::process::exit(hooks::run_hook_client());
    }
    // Kommandozeile lesen (nach `--hook`, das eigene Argumente von Claude Code mitbringt). Fehler: Exit-Code 2.
    let args: Vec<String> = std::env::args().collect();
    let cli = match cli::parse(&args, &cli::abs_from_cwd) {
        Ok(c) => c,
        Err(e) => {
            console::error(&e.message(early_lang(None, &cli::Cli::default())));
            std::process::exit(2);
        }
    };
    cli::set(cli);
    let cli = cli::get();
    // Vom Deinstallationsprogramm: die globalen Claude-Hooks wieder entfernen, sonst zeigen sie auf eine gelöschte .exe.
    if cli.uninstall_hooks {
        let ok = hooks::global_settings_path().map(|p| !p.exists() || hooks::uninstall_global(&p).is_ok()).unwrap_or(true);
        // Ebenso eine Registrierung als MCP-Server in Claude Code, falls sie auf diese .exe zeigt.
        mcp::unregister_if_ours();
        std::process::exit(if ok { 0 } else { 1 });
    }
    // Sprache für Hilfe und Meldungen: --lang > settings.json des Datenordners > Englisch.
    let lang = early_lang(cli.lang, cli);
    state::set_lang(lang);
    if cli.help {
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
    // Datenordner einmal auflösen; ein unbrauchbarer ausdrücklich gewählter Ordner ist ein Fehler (Exit-Code 1).
    let dir = match paths::init(cli) {
        Ok(d) => d.path.clone(),
        Err(e) => {
            console::error(&e.message(lang));
            std::process::exit(1);
        }
    };
    install_panic_hook();
    // Ein Update über eine laufende Instanz benennt die alte .exe in "paneldeck.exe.old" um (der Installer); jetzt weg damit.
    if let Ok(exe) = std::env::current_exe() {
        let mut old = exe.into_os_string();
        old.push(".old");
        let _ = std::fs::remove_file(old);
    }
    let cmd = match &cli.open {
        Some(p) => hooks::IpcCmd::Open { path: p.clone(), claude: cli.claude },
        None => hooks::IpcCmd::Show,
    };
    // `--reset-settings` nur, wenn keine Instanz mit diesem Datenordner läuft (sie würde die Datei wieder schreiben).
    if cli.reset_settings {
        // Die laufende Instanz hält die Instanzsperre (kein Weiterleiten: das Fenster soll nicht aufspringen).
        if acquire_instance_lock().is_err() {
            console::error(&format!(
                "Paneldeck: {} {}",
                state::t("--reset-settings wurde nicht ausgeführt: Paneldeck läuft bereits mit dem Datenordner", "--reset-settings was not applied: Paneldeck is already running with the data folder"),
                dir.display()
            ));
            std::process::exit(1);
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
    if acquire_instance_lock().is_err() {
        // Die andere Instanz startet gerade noch: ihren IPC-Server abwarten und an sie übergeben, statt Daten doppelt zu öffnen.
        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(250));
            if hooks::forward_to_running(&dir, &cmd) {
                break;
            }
        }
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Paneldeck")
            .with_inner_size([1320.0, 820.0])
            .with_min_inner_size([640.0, 400.0])
            .with_transparent(true)
            .with_decorations(false)
            .with_icon(app_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "Paneldeck",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
