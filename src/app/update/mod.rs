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
mod sha256;
mod ui;
pub(super) mod version;
mod zip;

// Für `main.rs` (TODO(2.1 wiring)): `app::update::wait_for_previous_instance()` vor der Einzelinstanz-Weiterleitung.
#[allow(unused_imports)]
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
    Downloaded(Result<PathBuf, String>),
    Installed(Result<(), String>),
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
    pub(super) dialog: bool,
    pub(super) confirm_restart: bool,
    /// Ergebnis der letzten Prüfung bzw. letzter Fehler (Text, ok).
    pub(super) result: Option<(String, bool)>,
    /// Fehler des Downloads/der Installation (im Dialog).
    pub(super) error: Option<String>,
    prepared: Option<PathBuf>,
    retry_at: i64,
    next_tick: Instant,
    flavor: Option<Option<Flavor>>,
}

impl Updater {
    pub fn new(auto_check_allowed: bool) -> Self {
        cleanup_old_downloads();
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
            dialog: false,
            confirm_restart: false,
            result: None,
            error: None,
            prepared: None,
            retry_at: 0,
            // Erste automatische Prüfung kurz nach dem Start (nicht im ersten Frame, damit der Start nicht wartet).
            next_tick: Instant::now() + Duration::from_secs(3),
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

    /// „Später“.
    pub(super) fn later(&mut self) {
        self.hidden = true;
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
        self.cancel.store(false, Ordering::Relaxed);
        self.done.store(0, Ordering::Relaxed);
        self.total.store(asset.size, Ordering::Relaxed);
        let (tx, ctx, cancel, done, total) = (self.tx.clone(), ctx.clone(), self.cancel.clone(), self.done.clone(), self.total.clone());
        std::thread::spawn(move || {
            let res = http::system_transport().and_then(|t| download(t.as_ref(), &asset, &sums, &version, &download_dir(), lang, &cancel, &mut |d, tot| {
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
        let (Some(file), Some(offer)) = (self.prepared.clone(), self.offer.clone()) else { return };
        if self.phase != Phase::Ready {
            return;
        }
        self.phase = Phase::Installing;
        self.error = None;
        let flavor = self.flavor(&exe);
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let res = install(&exe, flavor, &file, &offer, backup, lang);
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
                        self.confirm_restart = true;
                    }
                    Err(e) => {
                        self.phase = Phase::Idle;
                        self.error = Some(e);
                        self.dialog = true;
                    }
                },
                Msg::Installed(res) => match res {
                    Ok(()) => installed = true,
                    Err(e) => {
                        // Datei bleibt geprüft liegen: erneut versuchen geht ohne neuen Download.
                        self.phase = Phase::Ready;
                        self.error = Some(e);
                        self.dialog = true;
                    }
                },
            }
        }
        installed
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
    let t = http::system_transport()?;
    check_with(t.as_ref(), &Version::current(), install::detect(exe), skip, lang, cancel)
}

fn check_with(t: &dyn http::Transport, current: &Version, flavor: Option<Flavor>, skip: &str, lang: Lang, cancel: &AtomicBool) -> Result<Outcome, String> {
    let json = http::fetch_bytes(t, release::LATEST_URL, "application/vnd.github+json", release::MAX_JSON, cancel).map_err(|e| e.text(lang))?;
    let rel = release::parse_latest(&json).map_err(|e| format!("{}: {e}", lang.t("Unerwartete Antwort von GitHub", "Unexpected answer from GitHub")))?;
    Ok(release::evaluate(current, &rel, flavor, skip))
}

/// Ordner für die Downloads dieser Instanz.
fn download_dir() -> PathBuf {
    std::env::temp_dir().join(format!("paneldeck-update-{}", std::process::id()))
}

/// Alte Download-Ordner (älter als ein Tag) aus dem Temp-Ordner entfernen.
fn cleanup_old_downloads() {
    let Ok(rd) = std::fs::read_dir(std::env::temp_dir()) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.starts_with("paneldeck-update-") || name == format!("paneldeck-update-{}", std::process::id()) {
            continue;
        }
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|d| d > Duration::from_secs(DAY_SECS as u64));
        if old {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// Prüfsummen laden, Datei laden (mit Fortschritt) und prüfen; bei Abweichung wird die Datei gelöscht. ZIPs werden zusätzlich
/// probehalber entpackt, damit ein kaputtes Archiv vor der Neustart-Frage auffällt.
#[allow(clippy::too_many_arguments)]
fn download(
    t: &dyn http::Transport,
    asset: &release::Asset,
    sums: &release::Asset,
    version: &Version,
    dir: &Path,
    lang: Lang,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<PathBuf, String> {
    use std::io::Write;
    let text = http::fetch_bytes(t, &sums.url, "application/octet-stream", release::MAX_SUMS, cancel).map_err(|e| format!("{}: {}", release::SUMS_NAME, e.text(lang)))?;
    let list = release::parse_sums(&String::from_utf8_lossy(&text));
    let want = release::expected_hash(&list, &asset.name).ok_or_else(|| format!("{} {} {}", release::SUMS_NAME, lang.t("enthält keine Prüfsumme für", "has no checksum for"), asset.name))?;
    if asset.name.contains(['/', '\\']) || asset.name.contains("..") {
        return Err(format!("{}: {}", lang.t("Ungültiger Dateiname", "Invalid file name"), asset.name));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(&asset.name);
    let res = (|| {
        let mut f = std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut h = sha256::Sha256::new();
        http::fetch(t, &asset.url, "application/octet-stream", release::MAX_ASSET, cancel, progress, &mut |b| {
            h.update(b);
            f.write_all(b).map_err(|e| e.to_string())
        })
        .map_err(|e| e.text(lang))?;
        f.sync_all().map_err(|e| e.to_string())?;
        drop(f);
        if h.finish() != want {
            return Err(lang.t("Prüfsumme stimmt nicht – der Download wurde verworfen.", "Checksum mismatch – the download was discarded.").to_string());
        }
        if asset.name.ends_with(".zip") {
            let z = std::fs::read(&path).map_err(|e| e.to_string())?;
            check_exe(&zip::extract(&z, &release::zip_exe_entry(version), release::MAX_ASSET as usize).map_err(|e| zip_err(e, lang))?, lang)?;
        }
        Ok(())
    })();
    match res {
        Ok(()) => Ok(path),
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            Err(e)
        }
    }
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

/// Installieren: bei einer Hauptversion vorher die Datenbank sichern (scheitert das, wird nicht aktualisiert), dann Setup still
/// ausführen bzw. die Programmdatei tauschen.
fn install(exe: &Path, flavor: Option<Flavor>, file: &Path, offer: &Offer, backup: Option<(PathBuf, PathBuf)>, lang: Lang) -> Result<(), String> {
    if offer.major {
        if let Some((db, dir)) = backup {
            crate::backup::create(&db, &dir, crate::backup::Kind::Manual).map_err(|e| format!("{}: {e}", lang.t("Sicherung fehlgeschlagen – Update abgebrochen", "Backup failed – update cancelled")))?;
        }
    }
    match flavor {
        Some(Flavor::Installed) => {
            let dir = exe.parent().ok_or("exe")?;
            install::run_setup(file, dir, Duration::from_secs(300)).map_err(|e| format!("{}: {e}", lang.t("Installation fehlgeschlagen", "Installation failed")))
        }
        Some(Flavor::Portable) => {
            let z = std::fs::read(file).map_err(|e| e.to_string())?;
            let bytes = zip::extract(&z, &release::zip_exe_entry(&offer.version), release::MAX_ASSET as usize).map_err(|e| zip_err(e, lang))?;
            check_exe(&bytes, lang)?;
            install::swap_exe(exe, &bytes).map_err(|e| format!("{}: {e}", lang.t("paneldeck.exe konnte nicht ersetzt werden", "Could not replace paneldeck.exe")))
        }
        None => Err(lang.t("Automatische Updates gibt es nur unter Windows.", "Automatic updates are only available on Windows.").into()),
    }
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
        let p = download(&t, &asset, &sums, &v, &d.join("dl"), Lang::En, &AtomicBool::new(false), &mut |n, _| last = n).unwrap();
        assert!(p.is_file());
        assert!(last > 0);
        // Portabel installieren: exe getauscht, alte als .old
        let exe = d.join("paneldeck.exe");
        std::fs::write(&exe, b"MZ old").unwrap();
        let offer = Offer { version: v.clone(), html_url: String::new(), plan: Plan::Install { asset, sums }, major: false };
        install(&exe, Some(Flavor::Portable), &p, &offer, None, Lang::En).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), fake_exe());
        assert_eq!(std::fs::read(d.join("paneldeck.exe.old")).unwrap(), b"MZ old");
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
        assert!(!d.join(&asset.name).exists(), "verworfene Datei ist gelöscht");
        // Prüfsumme fehlt für die Datei
        let setup = rel.asset(&release::setup_name(&v)).unwrap().clone();
        let t2 = Fake::default().with(&sums.url, Canned::ok(b"".to_vec()));
        let e = download(&t2, &setup, &sums, &v, &d, Lang::En, &AtomicBool::new(false), &mut |_, _| {}).unwrap_err();
        assert!(e.contains("no checksum"), "{e}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn major_backup_failure_blocks_install() {
        let d = tmp("major");
        let exe = d.join("paneldeck.exe");
        std::fs::write(&exe, b"MZ old").unwrap();
        let v = Version::new(10, 0, 0);
        let a = release::Asset { name: "x".into(), url: String::new(), size: 0 };
        let offer = Offer { version: v, html_url: String::new(), plan: Plan::Install { asset: a.clone(), sums: a }, major: true };
        // Datenbank fehlt → Sicherung scheitert → nichts wird angefasst
        let e = install(&exe, Some(Flavor::Portable), &d.join("none.zip"), &offer, Some((d.join("missing.db"), d.join("backups"))), Lang::En).unwrap_err();
        assert!(e.contains("Backup failed"), "{e}");
        assert_eq!(std::fs::read(&exe).unwrap(), b"MZ old");
        // Mit echter Datenbank wird gesichert
        let db = d.join("paneldeck.db");
        rusqlite::Connection::open(&db).unwrap().execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (1);").unwrap();
        let _ = install(&exe, Some(Flavor::Portable), &d.join("none.zip"), &offer, Some((db, d.join("backups"))), Lang::En);
        assert_eq!(crate::backup::list(&d.join("backups")).len(), 1);
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
        // Installation fertig → Neustart verlangt
        u.tx.send(Msg::Installed(Ok(()))).unwrap();
        assert!(u.poll(&mut s));
    }
}
