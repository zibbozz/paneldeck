//! Selbst-Update aus den GitHub-Releases (Issue #113).
//!
//! Ablauf: Prüfen (Knopf unter *Einstellungen → Info* oder automatisch beim Start, höchstens einmal am Tag – Standard aus) →
//! neuere reguläre Version gefunden → Knopf in der Fensterleiste → Dialog („Jetzt aktualisieren“, „Später“, „Diese Version
//! überspringen“, „Was ist neu“) → Herunterladen samt Prüfung gegen `SHA256SUMS.txt` → bei laufenden Sitzungen nachfragen →
//! (Hauptversion: Datenbank sichern) → installieren (Setup still bzw. `paneldeck.exe` tauschen) → Zustand speichern, neu starten.
//! Alles Netzwerk läuft in Hintergrundfäden; die Oberfläche blockiert nie. Agenten (MCP) können kein Update auslösen.
//!
//! - `version.rs`: SemVer-Vergleich · `release.rs`: API-Antwort, Dateien, Prüfsummen, erlaubte Adressen, Entscheidung
//! - `http.rs`: Abruf mit Weiterleitungsprüfung (WinHTTP; Attrappe in Tests) · `sha256.rs`, `zip.rs`: Prüfsumme, ZIP lesen
//! - `install.rs`: Installationsart, Setup/Tausch, Neustart · `ui.rs`: Fensterleisten-Knopf, Dialoge, Abschnitt unter „Info“

pub(super) mod http;
pub mod install;
pub(super) mod release;
pub(crate) mod sha256;
mod ui;
pub(super) mod version;
mod zip;

// Für `main.rs`: vor der Einzelinstanz-Weiterleitung aufrufen.
pub use install::wait_for_previous_instance;

use crate::state::Lang;
use release::{Flavor, Offer, Outcome, Plan};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};
use version::Version;

/// Automatische Prüfung höchstens einmal in 24 Stunden.
pub const DAY_SECS: i64 = 24 * 3600;
/// Nach einer gescheiterten automatischen Prüfung (z. B. offline) frühestens nach einer Stunde erneut.
const RETRY_SECS: i64 = 3600;

pub fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Ist eine automatische Prüfung fällig? Nur wenn eingeschaltet, für diesen Start erlaubt (`--no-update-check`) und die letzte
/// erfolgreiche Prüfung mindestens 24 h her ist (eine Uhr, die zurückgestellt wurde, blockiert nicht dauerhaft).
pub fn auto_due(enabled: bool, allowed: bool, last_check: i64, retry_at: i64, now: i64) -> bool {
    enabled && allowed && now >= retry_at && (last_check <= 0 || now < last_check || now - last_check >= DAY_SECS)
}

/// Soll der Knopf in der Fensterleiste nach einer Prüfung erscheinen? Übersprungene Versionen nur bei manueller Prüfung.
pub fn show_after_check(manual: bool, skipped: bool) -> bool {
    manual || !skipped
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Phase {
    Idle,
    Checking { manual: bool },
    Downloading,
    /// Heruntergeladen und geprüft; wartet auf die Bestätigung des Neustarts.
    Ready,
    Installing,
}

enum Msg {
    Checked { manual: bool, res: Result<Outcome, String> },
    /// Geprüfte Datei samt erwarteter Prüfsumme (wird vor der Installation erneut verglichen).
    Downloaded(Result<(PathBuf, [u8; 32]), String>),
    Installed(Result<(), InstallFail>),
}

/// Zustand des Updates in der Oberfläche (ein Feld in `App`).
pub struct Updater {
    /// Automatische Prüfung für diesen Start erlaubt (`false` bei `--no-update-check`).
    allowed: bool,
    pub(super) phase: Phase,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
    /// Bekannte neuere Version.
    pub(super) offer: Option<Offer>,
    /// „Später“: Knopf bis zur nächsten Prüfung bzw. zum nächsten Start ausblenden.
    hidden: bool,
    /// „Später“ während des Downloads: danach nicht nachfragen, nur „Neu starten“ in der Fensterleiste zeigen.
    deferred: bool,
    pub(super) dialog: bool,
    pub(super) confirm_restart: bool,
    /// Ergebnis der letzten Prüfung bzw. letzter Fehler (Text, ok).
    pub(super) result: Option<(String, bool)>,
    /// Fehler des Downloads/der Installation (im Dialog).
    pub(super) error: Option<String>,
    /// Neutraler Hinweis im Dialog (z. B. Download abgebrochen).
    pub(super) info: Option<String>,
    /// Heruntergeladene, geprüfte Datei und ihre erwartete SHA-256-Prüfsumme.
    prepared: Option<(PathBuf, [u8; 32])>,
    retry_at: i64,
    next_tick: Instant,
    /// Gezählte Frames bis 2: nach dem ersten gezeichneten Frame wird `paneldeck.exe.old` gelöscht.
    frames: u8,
    flavor: Option<Option<Flavor>>,
}

impl Updater {
    pub fn new(auto_check_allowed: bool) -> Self {
        // Im Hintergrund: der Start soll nicht warten.
        std::thread::spawn(|| cleanup_old_downloads(&downloads_root()));
        let (tx, rx) = std::sync::mpsc::channel();
        Self {
            allowed: auto_check_allowed,
            phase: Phase::Idle,
            tx,
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
            done: Arc::new(AtomicU64::new(0)),
            total: Arc::new(AtomicU64::new(0)),
            offer: None,
            hidden: false,
            deferred: false,
            dialog: false,
            confirm_restart: false,
            result: None,
            error: None,
            info: None,
            prepared: None,
            retry_at: 0,
            // Erste automatische Prüfung kurz nach dem Start (nicht im ersten Frame, damit der Start nicht wartet).
            next_tick: Instant::now() + Duration::from_secs(3),
            frames: 0,
            flavor: None,
        }
    }

    /// Ist der Knopf in der Fensterleiste sichtbar?
    pub(super) fn button_visible(&self) -> bool {
        matches!(self.phase, Phase::Downloading | Phase::Ready | Phase::Installing) || (self.offer.is_some() && !self.hidden)
    }

    pub(super) fn busy(&self) -> bool {
        self.phase != Phase::Idle
    }

    /// Fortschritt des Downloads (geladen, gesamt; gesamt 0 = unbekannt).
    pub(super) fn progress(&self) -> (u64, u64) {
        (self.done.load(Ordering::Relaxed), self.total.load(Ordering::Relaxed))
    }

    pub(super) fn flavor(&mut self, exe: &Path) -> Option<Flavor> {
        *self.flavor.get_or_insert_with(|| install::detect(exe))
    }

    /// „Später“. Während des Downloads: weiterladen, danach nur den Knopf „Neu starten“ zeigen (keine Nachfrage).
    pub(super) fn later(&mut self) {
        if self.phase == Phase::Downloading {
            self.deferred = true;
        } else {
            self.hidden = true;
        }
        self.dialog = false;
    }

    /// Prüfung starten (im Hintergrund).
    pub(super) fn start_check(&mut self, manual: bool, exe: PathBuf, skip: String, lang: Lang, ctx: &eframe::egui::Context) {
        if self.busy() {
            return;
        }
        self.phase = Phase::Checking { manual };
        if manual {
            self.result = Some((lang.t("Suche nach Updates …", "Checking for updates …").into(), true));
        }
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        std::thread::spawn(move || {
            let res = check(&exe, &skip, lang, &cancel);
            let _ = tx.send(Msg::Checked { manual, res });
            ctx.request_repaint();
        });
    }

    /// Download der angebotenen Version starten.
    pub(super) fn start_download(&mut self, lang: Lang, ctx: &eframe::egui::Context) {
        let Some(Offer { plan: Plan::Install { asset, sums }, .. }) = self.offer.clone() else { return };
        if self.busy() {
            return;
        }
        let version = self.offer.as_ref().map(|o| o.version.clone()).unwrap_or(Version::new(0, 0, 0));
        self.phase = Phase::Downloading;
        self.error = None;
        self.info = None;
        self.deferred = false;
        self.cancel.store(false, Ordering::Relaxed);
        self.done.store(0, Ordering::Relaxed);
        self.total.store(asset.size, Ordering::Relaxed);
        let (tx, ctx, cancel, done, total) = (self.tx.clone(), ctx.clone(), self.cancel.clone(), self.done.clone(), self.total.clone());
        std::thread::spawn(move || {
            let res = http::system_transport().map_err(|e| e.text(lang)).and_then(|t| download(t.as_ref(), &asset, &sums, &version, &downloads_root(), lang, &cancel, &mut |d, tot| {
                done.store(d, Ordering::Relaxed);
                if let Some(t) = tot {
                    total.store(t, Ordering::Relaxed);
                }
            }));
            let _ = tx.send(Msg::Downloaded(res));
            ctx.request_repaint();
        });
    }

    pub(super) fn cancel_download(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Installation starten (nach der Bestätigung). `backup` = (Datenbank, Sicherungsordner) für eine Hauptversion.
    pub(super) fn start_install(&mut self, exe: PathBuf, backup: Option<(PathBuf, PathBuf)>, lang: Lang, ctx: &eframe::egui::Context) {
        let (Some((file, want)), Some(offer)) = (self.prepared.clone(), self.offer.clone()) else { return };
        if self.phase != Phase::Ready {
            return;
        }
        self.phase = Phase::Installing;
        self.error = None;
        let flavor = self.flavor(&exe);
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let res = install(&exe, flavor, &file, &want, &offer, backup, lang);
            let _ = tx.send(Msg::Installed(res));
            ctx.request_repaint();
        });
    }

    /// Nachrichten der Hintergrundfäden abholen. Liefert `true`, wenn die Installation fertig ist (→ neu starten).
    pub(super) fn poll(&mut self, settings: &mut crate::state::Settings) -> bool {
        let lang = settings.language;
        let mut installed = false;
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Checked { manual, res } => {
                    self.phase = Phase::Idle;
                    match res {
                        Ok(Outcome::UpToDate) => {
                            settings.update_last_check = now_unix();
                            self.offer = None;
                            self.result = Some((format!("{} ({})", lang.t("Paneldeck ist aktuell", "Paneldeck is up to date"), Version::current()), true));
                        }
                        Ok(Outcome::Newer { offer, skipped }) => {
                            settings.update_last_check = now_unix();
                            let mut text = format!("{} {} {}", lang.t("Version", "Version"), offer.version, lang.t("ist verfügbar", "is available"));
                            if skipped {
                                text.push_str(lang.t(" (übersprungen)", " (skipped)"));
                            }
                            self.result = Some((text, true));
                            if show_after_check(manual, skipped) {
                                self.hidden = false;
                                self.dialog |= manual;
                                if self.offer.as_ref().map(|o| &o.version) != Some(&offer.version) {
                                    self.prepared = None;
                                }
                                self.offer = Some(offer);
                            }
                        }
                        Err(e) => {
                            self.retry_at = now_unix() + RETRY_SECS;
                            self.result = Some((format!("{}: {e}", lang.t("Prüfung fehlgeschlagen", "Check failed")), false));
                        }
                    }
                }
                Msg::Downloaded(res) => match res {
                    Ok(p) => {
                        self.prepared = Some(p);
                        self.phase = Phase::Ready;
                        // Nie still neu starten: immer nachfragen – außer der Nutzer hat „Später“ gewählt (dann nur der Knopf).
                        self.confirm_restart = !self.deferred;
                    }
                    Err(_) if self.cancel.load(Ordering::Relaxed) => {
                        // Selbst abgebrochen: kein Fehler, nur ein Hinweis.
                        self.phase = Phase::Idle;
                        self.info = Some(lang.t("Download abgebrochen.", "Download cancelled.").into());
                    }
                    Err(e) => {
                        self.phase = Phase::Idle;
                        self.error = Some(e);
                        self.dialog = true;
                    }
                },
                Msg::Installed(res) => match res {
                    Ok(()) => installed = true,
                    Err(InstallFail::Retry(e)) => {
                        // Datei bleibt geprüft liegen: erneut versuchen geht ohne neuen Download.
                        self.phase = Phase::Ready;
                        self.error = Some(e);
                        self.dialog = true;
                    }
                    Err(InstallFail::Discard(e)) => {
                        // Datei fehlt/verändert: verwerfen, damit ein neuer Download möglich ist (sonst bliebe „Bereit“ hängen).
                        self.discard_prepared();
                        self.phase = Phase::Idle;
                        self.error = Some(e);
                        self.dialog = true;
                    }
                    Err(InstallFail::StillRunning(e)) => {
                        // Das Setup arbeitet noch: nichts erneut anbieten (kein zweites Setup), nur melden.
                        self.prepared = None;
                        self.offer = None;
                        self.phase = Phase::Idle;
                        self.error = Some(e);
                        self.dialog = true;
                    }
                },
            }
        }
        installed
    }

    /// Vorbereitete Datei vergessen und samt (dann leerem) Download-Ordner löschen.
    fn discard_prepared(&mut self) {
        if let Some((p, _)) = self.prepared.take() {
            let _ = std::fs::remove_file(&p);
            if let Some(d) = p.parent() {
                let _ = std::fs::remove_dir(d);
            }
        }
    }

    /// Pro Frame: `true` genau einmal, sobald der erste Frame gezeichnet ist (beim Aufruf im zweiten Frame).
    pub(super) fn first_frame_done(&mut self) -> bool {
        if self.frames >= 2 {
            return false;
        }
        self.frames += 1;
        self.frames == 2
    }

    /// Pro Frame (billig): ist eine automatische Prüfung fällig?
    pub(super) fn auto_tick(&mut self, settings: &crate::state::Settings) -> bool {
        if Instant::now() < self.next_tick || self.busy() {
            return false;
        }
        self.next_tick = Instant::now() + Duration::from_secs(60);
        auto_due(settings.update_auto_check, self.allowed, settings.update_last_check, self.retry_at, now_unix())
    }
}

/// Abfrage der API und Entscheidung.
fn check(exe: &Path, skip: &str, lang: Lang, cancel: &AtomicBool) -> Result<Outcome, String> {
    let t = http::system_transport().map_err(|e| e.text(lang))?;
    check_with(t.as_ref(), &Version::current(), install::detect(exe), skip, lang, cancel)
}

fn check_with(t: &dyn http::Transport, current: &Version, flavor: Option<Flavor>, skip: &str, lang: Lang, cancel: &AtomicBool) -> Result<Outcome, String> {
    let json = http::fetch_bytes(t, release::LATEST_URL, "application/vnd.github+json", release::MAX_JSON, cancel).map_err(|e| e.text(lang))?;
    let rel = release::parse_latest(&json).map_err(|e| format!("{}: {e}", lang.t("Unerwartete Antwort von GitHub", "Unexpected answer from GitHub")))?;
    Ok(release::evaluate(current, &rel, flavor, skip))
}

/// Übergeordneter Ordner der Downloads: `%LOCALAPPDATA%\Paneldeck\updates` (nicht der gemeinsame, vorhersagbare Temp-Ordner).
fn downloads_root() -> PathBuf {
    match std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty()) {
        Some(d) => PathBuf::from(d).join("Paneldeck").join("updates"),
        None => std::env::temp_dir().join("Paneldeck-updates"),
    }
}

/// Neuer, leerer Download-Ordner `<root>/<pid>-<zufall>`: mit `create_dir` angelegt, damit nie ein schon vorhandener
/// (womöglich fremd vorbereiteter) Ordner benutzt wird. Die PID im Namen braucht das Aufräumen (`cleanup_old_downloads`).
fn fresh_download_dir(root: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(root)?;
    let mut last = None;
    for _ in 0..8 {
        let mut r = [0u8; 8];
        if getrandom::fill(&mut r).is_err() {
            // Ersatz ohne Zufallsquelle: Zeit in Nanosekunden (create_dir schützt trotzdem vor vorhandenen Ordnern)
            let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
            r = n.to_le_bytes();
        }
        let d = root.join(format!("{}-{}", std::process::id(), r.iter().map(|b| format!("{b:02x}")).collect::<String>()));
        match std::fs::create_dir(&d) {
            Ok(()) => return Ok(d),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("download folder")))
}

/// Läuft der Prozess `pid` noch? Im Zweifel (z. B. kein Zugriff) ja – dann wird sein Ordner nicht angefasst.
fn pid_alive(pid: u32) -> bool {
    if pid == std::process::id() {
        return true;
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_INVALID_PARAMETER};
        use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        const STILL_ACTIVE: u32 = 259;
        // SAFETY: OpenProcess nur mit Abfragerecht; das Handle wird geschlossen. GetLastError liest nur den Fehlerwert.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                // ERROR_INVALID_PARAMETER = keine solche PID (mehr)
                return GetLastError() != ERROR_INVALID_PARAMETER;
            }
            let mut code = 0u32;
            let ok = GetExitCodeProcess(h, &mut code) != 0;
            CloseHandle(h);
            !ok || code == STILL_ACTIVE
        }
    }
    #[cfg(not(windows))]
    {
        Path::new(&format!("/proc/{pid}")).exists()
    }
}

/// Download-Ordner beendeter Instanzen entfernen. Ordner laufender Instanzen (auch anderer Datenordner) und alles, was nicht
/// wie `<pid>-<zufall>` heißt, bleiben unberührt.
fn cleanup_old_downloads(root: &Path) {
    let Ok(rd) = std::fs::read_dir(root) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let pid = name
            .split_once('-')
            .filter(|(_, rest)| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_hexdigit()))
            .and_then(|(p, _)| p.parse::<u32>().ok());
        let Some(pid) = pid else { continue };
        if e.file_type().is_ok_and(|t| t.is_dir()) && !pid_alive(pid) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// Prüfsummen laden, Datei in einen neuen Ordner unter `root` laden (mit Fortschritt) und prüfen; bei Abweichung wird der Ordner
/// samt Datei gelöscht. ZIPs werden zusätzlich probehalber entpackt, damit ein kaputtes Archiv vor der Neustart-Frage auffällt.
#[allow(clippy::too_many_arguments)]
fn download(
    t: &dyn http::Transport,
    asset: &release::Asset,
    sums: &release::Asset,
    version: &Version,
    root: &Path,
    lang: Lang,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<(PathBuf, [u8; 32]), String> {
    use std::io::Write;
    let text = http::fetch_bytes(t, &sums.url, "application/octet-stream", release::MAX_SUMS, cancel)
        .map_err(|e| format!("{} ({}): {}", lang.t("Prüfsummen-Datei nicht geladen", "Could not load the checksum file"), release::SUMS_NAME, e.text(lang)))?;
    let list = release::parse_sums(&String::from_utf8_lossy(&text));
    let want = release::expected_hash(&list, &asset.name).ok_or_else(|| {
        format!("{} ({}: {})", lang.t("Die Prüfsummen-Datei enthält keine Prüfsumme für diese Datei", "The checksum file has no checksum for this file"), release::SUMS_NAME, asset.name)
    })?;
    if asset.name.contains(['/', '\\']) || asset.name.contains("..") {
        return Err(format!("{} ({})", lang.t("Ungültiger Dateiname im Release", "Invalid file name in the release"), asset.name));
    }
    let save_err = |e: &dyn std::fmt::Display, p: &Path| format!("{} ({}: {e})", lang.t("Download konnte nicht gespeichert werden", "Could not save the download"), p.display());
    let dir = fresh_download_dir(root).map_err(|e| save_err(&e, root))?;
    let path = dir.join(&asset.name);
    let res = (|| {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(|e| save_err(&e, &path))?;
        let mut h = sha256::Sha256::new();
        http::fetch(t, &asset.url, "application/octet-stream", release::MAX_ASSET, cancel, progress, &mut |b| {
            h.update(b);
            f.write_all(b).map_err(|e| save_err(&e, &path))
        })
        .map_err(|e| e.text(lang))?;
        f.sync_all().map_err(|e| save_err(&e, &path))?;
        drop(f);
        if h.finish() != want {
            return Err(lang.t("Prüfsumme stimmt nicht – der Download wurde verworfen.", "Checksum mismatch – the download was discarded.").to_string());
        }
        if asset.name.ends_with(".zip") {
            let z = std::fs::read(&path).map_err(|e| save_err(&e, &path))?;
            check_exe(&zip::extract(&z, &release::zip_exe_entry(version), release::MAX_ASSET as usize).map_err(|e| zip_err(e, lang))?, lang)?;
        }
        Ok(())
    })();
    match res {
        Ok(()) => Ok((path, want)),
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dir);
            Err(e)
        }
    }
}

/// Mitgelieferte Dateien neben `paneldeck.exe` (ConPTY) aus dem geprüften ZIP ersetzen – nur als Paar und gemeinsam
/// (`install::swap_files`, alles oder nichts). Läuft vor dem Tausch der .exe und nur nach bestem Bemühen: fehlt eine Datei,
/// ist sie unbrauchbar oder klappt der Tausch nicht, bleiben die bisherigen (bzw. Paneldeck nutzt die ConPTY von Windows).
/// Eine neue ConPTY neben der alten .exe ist unbedenklich, falls danach der Tausch der .exe scheitert.
fn swap_companions(exe: &Path, zip_data: &[u8], v: &Version) {
    let Some(dir) = exe.parent() else { return };
    let mut items = Vec::new();
    for name in install::COMPANIONS {
        match zip::extract(zip_data, &release::zip_entry(v, name), release::MAX_ASSET as usize) {
            Ok(bytes) if bytes.len() >= 1024 && bytes.starts_with(b"MZ") => items.push((dir.join(name), bytes)),
            _ => return,
        }
    }
    let _ = install::swap_files(&items);
}

fn zip_err(e: String, lang: Lang) -> String {
    format!("{} ({e})", lang.t("Das ZIP ist beschädigt oder unvollständig", "The ZIP is damaged or incomplete"))
}

fn check_exe(b: &[u8], lang: Lang) -> Result<(), String> {
    if b.len() < 1024 || &b[..2] != b"MZ" {
        return Err(lang.t("Das ZIP enthält keine gültige paneldeck.exe.", "The ZIP does not contain a valid paneldeck.exe.").into());
    }
    Ok(())
}

/// Wie lange auf das Setup gewartet wird (danach läuft es weiter, Paneldeck wartet nur nicht mehr).
const SETUP_WAIT: Duration = Duration::from_secs(300);

/// Fehlschlag der Installation – und was danach gilt.
#[derive(Debug, PartialEq)]
pub(super) enum InstallFail {
    /// Die geprüfte Datei bleibt liegen: erneut versuchen geht ohne neuen Download (Sicherung, Setup-Code, Tausch).
    Retry(String),
    /// Die Datei fehlt, wurde verändert oder ist unbrauchbar: verworfen, ein neuer Download ist nötig.
    Discard(String),
    /// Das Setup läuft nach der Wartezeit noch: nicht abgebrochen, nur gemeldet.
    StillRunning(String),
}

/// Installieren: die heruntergeladene Datei gesperrt öffnen (nur Lesen bleibt anderen erlaubt) und erneut gegen die Prüfsumme
/// vergleichen, bei einer Hauptversion vorher die Datenbank sichern (scheitert das, wird nicht aktualisiert), dann Setup still
/// ausführen (die Sperre hält, bis es endet) bzw. die Programmdatei tauschen.
#[allow(clippy::too_many_arguments)]
fn install(exe: &Path, flavor: Option<Flavor>, file: &Path, want: &[u8; 32], offer: &Offer, backup: Option<(PathBuf, PathBuf)>, lang: Lang) -> Result<(), InstallFail> {
    use InstallFail::*;
    let (data, lock) = install::read_locked(file).map_err(|e| {
        Discard(format!("{} ({}: {e})", lang.t("Die heruntergeladene Datei fehlt – bitte erneut aktualisieren", "The downloaded file is missing – please update again"), file.display()))
    })?;
    if sha256::digest(&data) != *want {
        drop(lock);
        let _ = std::fs::remove_file(file);
        return Err(Discard(lang.t("Die heruntergeladene Datei wurde verändert – bitte erneut aktualisieren.", "The downloaded file was changed – please update again.").into()));
    }
    if offer.major {
        if let Some((db, dir)) = backup {
            crate::backup::create(&db, &dir, crate::backup::Kind::Manual).map_err(|e| Retry(format!("{} ({e})", lang.t("Sicherung fehlgeschlagen – Update abgebrochen", "Backup failed – update cancelled"))))?;
        }
    }
    let res = match flavor {
        Some(Flavor::Installed) => {
            let fail = lang.t("Installation fehlgeschlagen", "Installation failed");
            let dir = exe.parent().ok_or_else(|| Retry(format!("{fail} ({})", lang.t("Programmordner nicht gefunden", "program folder not found"))))?;
            match install::run_setup(file, dir, SETUP_WAIT) {
                Ok(()) => Ok(()),
                Err(install::SetupError::StillRunning) => Err(StillRunning(
                    lang.t(
                        "Das Setup läuft nach 5 Minuten noch. Paneldeck wartet nicht länger – bitte später unter Einstellungen → Info prüfen, ob die neue Version installiert ist, und Paneldeck dann selbst neu starten.",
                        "The setup is still running after 5 minutes. Paneldeck stops waiting – please check later under Settings → About whether the new version is installed, then restart Paneldeck yourself.",
                    )
                    .into(),
                )),
                Err(install::SetupError::Exit(c)) => Err(Retry(format!("{fail} ({} {c})", lang.t("Setup endete mit Code", "setup exit code")))),
                Err(install::SetupError::Start(e) | install::SetupError::Wait(e)) => Err(Retry(format!("{fail} ({e})"))),
            }
        }
        Some(Flavor::Portable) => {
            let bytes = zip::extract(&data, &release::zip_exe_entry(&offer.version), release::MAX_ASSET as usize).map_err(|e| Discard(zip_err(e, lang)))?;
            check_exe(&bytes, lang).map_err(Discard)?;
            swap_companions(exe, &data, &offer.version);
            install::swap_exe(exe, &bytes).map_err(|e| Retry(format!("{} ({e})", lang.t("paneldeck.exe konnte nicht ersetzt werden", "Could not replace paneldeck.exe"))))
        }
        None => Err(Discard(lang.t("Automatische Updates gibt es nur unter Windows.", "Automatic updates are only available on Windows.").into())),
    };
    // Sperre erst hier lösen: das Setup ist beendet (oder läuft nach der Wartezeit allein weiter).
    drop(lock);
    res
}

#[cfg(test)]
mod tests {
    use super::http::fake::{Canned, Fake};
    use super::*;

    #[test]
    fn schedule() {
        let now = 1_800_000_000;
        // Aus oder per --no-update-check unterdrückt: nie
        assert!(!auto_due(false, true, 0, 0, now));
        assert!(!auto_due(true, false, 0, 0, now));
        // Noch nie geprüft: sofort
        assert!(auto_due(true, true, 0, 0, now));
        // Höchstens einmal am Tag
        assert!(!auto_due(true, true, now - DAY_SECS + 60, 0, now));
        assert!(auto_due(true, true, now - DAY_SECS, 0, now));
        // Uhr zurückgestellt: nicht dauerhaft blockiert
        assert!(auto_due(true, true, now + 3600, 0, now));
        // Nach einem Fehler eine Stunde Pause
        assert!(!auto_due(true, true, 0, now + 10, now));
        assert!(auto_due(true, true, 0, now, now));
    }

    #[test]
    fn skip_logic() {
        assert!(show_after_check(false, false));
        assert!(!show_after_check(false, true), "automatisch: übersprungene Version nicht anzeigen");
        assert!(show_after_check(true, true), "manuell: trotzdem anzeigen");
    }

    fn feed(tag: &str, sums: bool) -> String {
        super::release::tests::fixture(tag, sums)
    }

    #[test]
    fn check_against_fake_api() {
        let t = Fake::default().with(release::LATEST_URL, Canned::ok(feed("v9.1.0", true)));
        let cancel = AtomicBool::new(false);
        let out = check_with(&t, &Version::new(9, 0, 0), Some(Flavor::Portable), "", Lang::En, &cancel).unwrap();
        let Outcome::Newer { offer, skipped: false } = out else { panic!() };
        assert_eq!(offer.version, Version::new(9, 1, 0));
        assert!(check_with(&t, &Version::new(9, 1, 0), Some(Flavor::Portable), "", Lang::En, &cancel).unwrap() == Outcome::UpToDate);
        let t = Fake::default().with(release::LATEST_URL, Canned::status(403));
        assert!(check_with(&t, &Version::new(9, 0, 0), None, "", Lang::En, &cancel).unwrap_err().contains("403"));
        let t = Fake::default().with(release::LATEST_URL, Canned::ok(b"<html>".to_vec()));
        assert!(check_with(&t, &Version::new(9, 0, 0), None, "", Lang::De, &cancel).unwrap_err().contains("GitHub"));
    }

    fn tmp(tag: &str) -> PathBuf {
        static C: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!("paneldeck-test-upddl-{tag}-{}-{}", std::process::id(), C.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn fake_exe() -> Vec<u8> {
        b"MZ".iter().copied().chain((0..4000u32).map(|i| (i * 7 % 256) as u8)).collect()
    }

    /// Fake-Release 9.1.0 mit ZIP und Prüfsummen; `corrupt` = ZIP nach dem Erzeugen der Prüfsummen verändern.
    fn fake_release(corrupt: bool) -> (Fake, release::Release) {
        let v = Version::new(9, 1, 0);
        let zip = zip::tests::make_zip(&[("paneldeck-9.1.0/paneldeck.exe", &fake_exe(), true), ("paneldeck-9.1.0/LICENSE", b"MIT", false)]);
        let sums = format!("{}  paneldeck-9.1.0-windows-x64.zip\n{}  Paneldeck-Setup-9.1.0.exe\n", sha256::hex(&sha256::digest(&zip)), "0".repeat(64));
        let mut served = zip.clone();
        if corrupt {
            let n = served.len() / 2;
            served[n] ^= 1;
        }
        let rel = release::parse_latest(feed("v9.1.0", true).as_bytes()).unwrap();
        let cdn = "https://release-assets.githubusercontent.com/x/zip?sig=1";
        let t = Fake::default()
            .with(&rel.asset(release::SUMS_NAME).unwrap().url, Canned::ok(sums.into_bytes()))
            .with(&rel.asset(&release::zip_name(&v)).unwrap().url, Canned::redirect(cdn))
            .with(cdn, Canned::ok(served));
        (t, rel)
    }

    #[test]
    fn download_verifies_and_swaps_portable() {
        let (t, rel) = fake_release(false);
        let d = tmp("ok");
        let v = Version::new(9, 1, 0);
        let asset = rel.asset(&release::zip_name(&v)).unwrap().clone();
        let sums = rel.asset(release::SUMS_NAME).unwrap().clone();
        let mut last = 0;
        let (p, want) = download(&t, &asset, &sums, &v, &d.join("dl"), Lang::En, &AtomicBool::new(false), &mut |n, _| last = n).unwrap();
        assert!(p.is_file());
        assert!(last > 0);
        // Portabel installieren: exe getauscht, alte als .old
        let exe = d.join("paneldeck.exe");
        std::fs::write(&exe, b"MZ old").unwrap();
        let offer = Offer { version: v.clone(), plan: Plan::Install { asset, sums }, major: false };
        install(&exe, Some(Flavor::Portable), &p, &want, &offer, None, Lang::En).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), fake_exe());
        assert_eq!(std::fs::read(d.join("paneldeck.exe.old")).unwrap(), b"MZ old");
        // Dieses ZIP enthält keine ConPTY: nichts angelegt.
        assert!(!d.join("conpty.dll").exists() && !d.join("OpenConsole.exe").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn portable_update_swaps_companions() {
        let d = tmp("companions");
        let v = Version::new(9, 1, 0);
        let dll: Vec<u8> = b"MZ dll".iter().copied().chain(std::iter::repeat_n(1u8, 2000)).collect();
        let host: Vec<u8> = b"MZ host".iter().copied().chain(std::iter::repeat_n(2u8, 2000)).collect();
        let zip = zip::tests::make_zip(&[
            ("paneldeck-9.1.0/paneldeck.exe", &fake_exe(), true),
            ("paneldeck-9.1.0/conpty.dll", &dll, true),
            ("paneldeck-9.1.0/OpenConsole.exe", &host, false),
            ("paneldeck-9.1.0/LICENSE", b"MIT", false),
        ]);
        let exe = d.join("paneldeck.exe");
        std::fs::write(&exe, b"MZ old").unwrap();
        // Vorhandene ConPTY wird getauscht (alte als .old), fehlende OpenConsole.exe neu angelegt.
        std::fs::write(d.join("conpty.dll"), b"MZ old dll").unwrap();
        swap_companions(&exe, &zip, &v);
        assert_eq!(std::fs::read(d.join("conpty.dll")).unwrap(), dll);
        assert_eq!(std::fs::read(d.join("conpty.dll.old")).unwrap(), b"MZ old dll");
        assert_eq!(std::fs::read(d.join("OpenConsole.exe")).unwrap(), host);
        // Ungültiger Inhalt (kein MZ / zu klein) wird nicht übernommen, und nur als Paar: auch die gültige zweite Datei nicht.
        let host2: Vec<u8> = b"MZ host2".iter().copied().chain(std::iter::repeat_n(3u8, 2000)).collect();
        let bad = zip::tests::make_zip(&[("paneldeck-9.1.0/conpty.dll", b"nope", false), ("paneldeck-9.1.0/OpenConsole.exe", &host2, false)]);
        swap_companions(&exe, &bad, &v);
        assert_eq!(std::fs::read(d.join("conpty.dll")).unwrap(), dll);
        assert_eq!(std::fs::read(d.join("OpenConsole.exe")).unwrap(), host);
        // Nur eine der beiden Dateien im ZIP: nichts getauscht.
        let half = zip::tests::make_zip(&[("paneldeck-9.1.0/OpenConsole.exe", &host2, false)]);
        swap_companions(&exe, &half, &v);
        assert_eq!(std::fs::read(d.join("OpenConsole.exe")).unwrap(), host);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn install_rechecks_downloaded_file() {
        let (t, rel) = fake_release(false);
        let d = tmp("recheck");
        let v = Version::new(9, 1, 0);
        let asset = rel.asset(&release::zip_name(&v)).unwrap().clone();
        let sums = rel.asset(release::SUMS_NAME).unwrap().clone();
        let (p, want) = download(&t, &asset, &sums, &v, &d.join("dl"), Lang::En, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
        // Zwischen Download und Neustart verändert → nichts wird angefasst, Datei verworfen
        let mut b = std::fs::read(&p).unwrap();
        b[10] ^= 1;
        std::fs::write(&p, b).unwrap();
        let exe = d.join("paneldeck.exe");
        std::fs::write(&exe, b"MZ old").unwrap();
        let offer = Offer { version: v, plan: Plan::Install { asset, sums }, major: false };
        let e = install(&exe, Some(Flavor::Portable), &p, &want, &offer, None, Lang::En).unwrap_err();
        assert!(matches!(&e, InstallFail::Discard(t) if t.contains("was changed")), "{e:?}");
        assert_eq!(std::fs::read(&exe).unwrap(), b"MZ old");
        assert!(!p.exists());
        // Datei fehlt ganz: ebenfalls verwerfen (neuer Download nötig)
        let e = install(&exe, Some(Flavor::Portable), &p, &want, &offer, None, Lang::En).unwrap_err();
        assert!(matches!(&e, InstallFail::Discard(t) if t.contains("missing")), "{e:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn download_mismatch_discards_file() {
        let (t, rel) = fake_release(true);
        let d = tmp("bad");
        let v = Version::new(9, 1, 0);
        let asset = rel.asset(&release::zip_name(&v)).unwrap().clone();
        let sums = rel.asset(release::SUMS_NAME).unwrap().clone();
        let e = download(&t, &asset, &sums, &v, &d, Lang::En, &AtomicBool::new(false), &mut |_, _| {}).unwrap_err();
        assert!(e.contains("Checksum mismatch"), "{e}");
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0, "verworfene Datei samt Ordner ist gelöscht");
        // Prüfsumme fehlt für die Datei
        let setup = rel.asset(&release::setup_name(&v)).unwrap().clone();
        let t2 = Fake::default().with(&sums.url, Canned::ok(b"".to_vec()));
        let e = download(&t2, &setup, &sums, &v, &d, Lang::En, &AtomicBool::new(false), &mut |_, _| {}).unwrap_err();
        assert!(e.contains("no checksum for this file"), "{e}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn download_folders_are_fresh_and_cleaned_per_process() {
        let root = tmp("dirs");
        let a = fresh_download_dir(&root).unwrap();
        let b = fresh_download_dir(&root).unwrap();
        assert_ne!(a, b, "jeder Download bekommt einen neuen Ordner");
        let name = a.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with(&format!("{}-", std::process::id())), "{name}");
        // Ordner eines beendeten Prozesses wird entfernt, eigene und fremde (laufende) bleiben, ebenso Unbekanntes
        let mut child = std::process::Command::new(if cfg!(windows) { "cmd" } else { "true" }).args(if cfg!(windows) { &["/c", "exit"][..] } else { &[][..] }).spawn().unwrap();
        let dead = child.id();
        child.wait().unwrap();
        drop(child);
        let dead_dir = root.join(format!("{dead}-00ff"));
        std::fs::create_dir(&dead_dir).unwrap();
        std::fs::write(dead_dir.join("x.zip"), b"x").unwrap();
        let other = root.join("notes-folder");
        std::fs::create_dir(&other).unwrap();
        let mut running = if cfg!(windows) {
            std::process::Command::new("cmd").args(["/c", "ping", "-n", "30", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().unwrap()
        } else {
            std::process::Command::new("sleep").arg("30").spawn().unwrap()
        };
        let parent_pid = root.join(format!("{}-abcd", running.id()));
        std::fs::create_dir(&parent_pid).unwrap();
        cleanup_old_downloads(&root);
        assert!(!dead_dir.exists(), "beendeter Prozess: aufgeräumt");
        assert!(a.exists() && b.exists(), "eigene Ordner bleiben");
        assert!(other.exists(), "fremd benannter Ordner bleibt");
        assert!(parent_pid.exists(), "Ordner einer laufenden Instanz bleibt");
        let _ = running.kill();
        let _ = running.wait();
        // Vorhandene Datei wird nie überschrieben (create_new)
        let (t, rel) = fake_release(false);
        let v = Version::new(9, 1, 0);
        let asset = rel.asset(&release::zip_name(&v)).unwrap().clone();
        let sums = rel.asset(release::SUMS_NAME).unwrap().clone();
        let (p, _) = download(&t, &asset, &sums, &v, &root, Lang::En, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
        assert!(p.starts_with(&root) && p.parent() != Some(a.as_path()) && p.parent() != Some(b.as_path()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn major_backup_failure_blocks_install() {
        let d = tmp("major");
        let exe = d.join("paneldeck.exe");
        std::fs::write(&exe, b"MZ old").unwrap();
        let v = Version::new(10, 0, 0);
        let a = release::Asset { name: "x".into(), url: String::new(), size: 0 };
        let offer = Offer { version: v, plan: Plan::Install { asset: a.clone(), sums: a }, major: true };
        // Eine (unbrauchbare) heruntergeladene Datei mit passender Prüfsumme
        let file = d.join("none.zip");
        std::fs::write(&file, b"not a zip").unwrap();
        let want = sha256::digest(b"not a zip");
        // Datenbank fehlt → Sicherung scheitert → nichts wird angefasst
        let e = install(&exe, Some(Flavor::Portable), &file, &want, &offer, Some((d.join("missing.db"), d.join("backups"))), Lang::En).unwrap_err();
        assert!(matches!(&e, InstallFail::Retry(t) if t.contains("Backup failed")), "{e:?}");
        assert_eq!(std::fs::read(&exe).unwrap(), b"MZ old");
        // Mit echter Datenbank wird gesichert
        let db = d.join("paneldeck.db");
        rusqlite::Connection::open(&db).unwrap().execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (1);").unwrap();
        let _ = install(&exe, Some(Flavor::Portable), &file, &want, &offer, Some((db, d.join("backups"))), Lang::En);
        assert_eq!(crate::backup::list(&d.join("backups")).len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Lokaler Schein-Release-Feed von `tools/update-feed.sh` (nur von Hand, kein Netz):
    /// `PANELDECK_TEST_FEED_DIR=<feed> cargo test --release local_feed -- --ignored`. Prüft API-Antwort, Prüfsummen, Download
    /// beider Dateien und den portablen Tausch gegen das echte ZIP (aus Pythons `zipfile`).
    #[test]
    #[ignore = "needs a feed from tools/update-feed.sh"]
    fn local_feed() {
        let dir = PathBuf::from(std::env::var_os("PANELDECK_TEST_FEED_DIR").expect("PANELDECK_TEST_FEED_DIR"));
        let t = http::feed::FeedTransport { dir };
        let cancel = AtomicBool::new(false);
        let d = tmp("feed");
        for flavor in [Flavor::Portable, Flavor::Installed] {
            let Outcome::Newer { offer, .. } = check_with(&t, &Version::new(0, 0, 1), Some(flavor), "", Lang::En, &cancel).unwrap() else { panic!("no offer") };
            let Plan::Install { asset, sums } = &offer.plan else { panic!("announce only: {:?}", offer.plan) };
            let (p, want) = download(&t, asset, sums, &offer.version, &d.join("dl"), Lang::En, &cancel, &mut |_, _| {}).unwrap();
            if flavor == Flavor::Portable {
                let exe = d.join("paneldeck.exe");
                std::fs::write(&exe, b"MZ old").unwrap();
                install(&exe, Some(flavor), &p, &want, &offer, None, Lang::En).unwrap();
                assert_eq!(&std::fs::read(&exe).unwrap()[..2], b"MZ");
                assert!(d.join("paneldeck.exe.old").is_file());
            }
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Echter Abruf über WinHTTP (nur von Hand: `cargo test --release live_github -- --ignored`). Liest `releases/latest` und lädt
    /// das Setup des neuesten Releases über die Weiterleitung zu `release-assets.githubusercontent.com`.
    #[test]
    #[ignore = "network"]
    fn live_github() {
        let t = http::system_transport().unwrap();
        let cancel = AtomicBool::new(false);
        let json = http::fetch_bytes(t.as_ref(), release::LATEST_URL, "application/vnd.github+json", release::MAX_JSON, &cancel).unwrap();
        let rel = release::parse_latest(&json).unwrap();
        let a = rel.asset(&release::setup_name(&rel.version)).expect("setup asset").clone();
        let mut h = sha256::Sha256::new();
        let mut first = Vec::new();
        let n = http::fetch(t.as_ref(), &a.url, "application/octet-stream", release::MAX_ASSET, &cancel, &mut |_, _| {}, &mut |b| {
            if first.len() < 2 {
                first.extend_from_slice(&b[..b.len().min(2)]);
            }
            h.update(b);
            Ok(())
        })
        .unwrap();
        assert_eq!(n, a.size);
        assert_eq!(&first[..2], b"MZ");
        eprintln!("{} {} {}", rel.tag, a.name, sha256::hex(&h.finish()));
    }

    #[test]
    fn updater_poll_flow() {
        let mut s = crate::state::Settings::default();
        let mut u = Updater::new(true);
        assert!(!u.button_visible());
        let rel = release::parse_latest(feed("v99.0.0", true).as_bytes()).unwrap();
        let out = release::evaluate(&Version::current(), &rel, Some(Flavor::Portable), "");
        // Automatische Prüfung: Knopf ja, Dialog nein
        u.tx.send(Msg::Checked { manual: false, res: Ok(out.clone()) }).unwrap();
        assert!(!u.poll(&mut s));
        assert!(u.button_visible() && !u.dialog);
        assert!(s.update_last_check > 0);
        u.later();
        assert!(!u.button_visible());
        // Übersprungen + automatisch: bleibt verborgen; manuell: Dialog öffnet sich
        let skipped = release::evaluate(&Version::current(), &rel, Some(Flavor::Portable), "99.0.0");
        u.tx.send(Msg::Checked { manual: false, res: Ok(skipped.clone()) }).unwrap();
        u.poll(&mut s);
        assert!(!u.button_visible());
        u.tx.send(Msg::Checked { manual: true, res: Ok(skipped) }).unwrap();
        u.poll(&mut s);
        assert!(u.button_visible() && u.dialog);
        // Fehler: Meldung, keine Zeit gespeichert, Wiederholung frühestens in einer Stunde
        s.update_last_check = 0;
        u.tx.send(Msg::Checked { manual: false, res: Err("offline".into()) }).unwrap();
        u.poll(&mut s);
        assert_eq!(s.update_last_check, 0);
        assert!(!u.result.as_ref().unwrap().1);
        assert!(u.retry_at > now_unix());
        // Download fertig: Nachfrage (nie still neu starten); nach „Später“ während des Downloads nur der Knopf
        let f0 = std::env::temp_dir().join(format!("paneldeck-test-ready-{}.zip", std::process::id()));
        u.phase = Phase::Downloading;
        u.tx.send(Msg::Downloaded(Ok((f0.clone(), [0; 32])))).unwrap();
        u.poll(&mut s);
        assert!(u.phase == Phase::Ready && u.confirm_restart);
        u.confirm_restart = false;
        u.phase = Phase::Downloading;
        u.later();
        assert!(u.deferred && !u.dialog);
        u.tx.send(Msg::Downloaded(Ok((f0, [0; 32])))).unwrap();
        u.poll(&mut s);
        assert!(u.phase == Phase::Ready && !u.confirm_restart && u.button_visible());
        // Abgebrochener Download: neutraler Hinweis statt Fehler
        u.phase = Phase::Downloading;
        u.error = None;
        u.cancel_download();
        u.tx.send(Msg::Downloaded(Err("Cancelled.".into()))).unwrap();
        u.poll(&mut s);
        assert_eq!(u.phase, Phase::Idle);
        assert!(u.error.is_none() && u.info.as_deref() == Some("Download cancelled."));
        u.cancel.store(false, Ordering::Relaxed);
        // Installation: Fehler mit noch gültiger Datei → erneut versuchen ohne Download
        let f = std::env::temp_dir().join(format!("paneldeck-test-prepared-{}.zip", std::process::id()));
        std::fs::write(&f, b"x").unwrap();
        u.prepared = Some((f.clone(), [0; 32]));
        u.phase = Phase::Installing;
        u.tx.send(Msg::Installed(Err(InstallFail::Retry("setup exit code 2".into())))).unwrap();
        assert!(!u.poll(&mut s));
        assert_eq!(u.phase, Phase::Ready);
        assert!(u.prepared.is_some() && u.dialog);
        // Datei fehlt/verändert → verworfen, neuer Download möglich (nicht in „Bereit“ hängen)
        u.phase = Phase::Installing;
        u.tx.send(Msg::Installed(Err(InstallFail::Discard("changed".into())))).unwrap();
        u.poll(&mut s);
        assert_eq!(u.phase, Phase::Idle);
        assert!(u.prepared.is_none() && !f.exists());
        // Setup läuft noch → nichts mehr anbieten
        let Outcome::Newer { offer, .. } = out else { panic!() };
        u.offer = Some(offer);
        u.phase = Phase::Installing;
        u.tx.send(Msg::Installed(Err(InstallFail::StillRunning("still running".into())))).unwrap();
        u.poll(&mut s);
        assert_eq!(u.phase, Phase::Idle);
        assert!(u.offer.is_none() && u.error.as_deref() == Some("still running"));
        // Installation fertig → Neustart verlangt
        u.tx.send(Msg::Installed(Ok(()))).unwrap();
        assert!(u.poll(&mut s));
    }
}
