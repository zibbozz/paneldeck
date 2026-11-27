//! Änderungen geöffneter Dateien auf der Platte erkennen (#54): einfache Abfrage von Größe und Änderungszeit, kein
//! Dateisystem-Beobachter (keine neue Abhängigkeit, funktioniert überall gleich). Die Abfrage läuft in einem eigenen
//! Thread – eine hängende Netzwerkfreigabe, die der Nutzer selbst geöffnet hat, blockiert so nie die Oberfläche.
//!
//! Ablauf je Bild: [`DiskWatch::tick`] mit den Pfaden der offenen Dokumente. Höchstens alle [`POLL_INTERVAL`] schickt
//! sie die Liste an den Thread; dessen Messungen vergleicht sie mit dem zuletzt bekannten Stand. Gemeldet wird eine
//! Änderung erst, wenn der neue Stand **zwei Abfragen lang gleich** bleibt ([`DiskEvent`]) – so liest der Editor keine
//! halb geschriebene Datei (z. B. während ein Agent sie schreibt). Nach dem eigenen Laden/Speichern ruft der Editor
//! [`DiskWatch::baseline`], damit das eigene Schreiben nicht als fremde Änderung zählt.
//!
//! Was mit einer Meldung passiert, entscheidet der Editor (Kern: ungespeicherte Änderungen? → Infoleiste *Meine
//! behalten* / *Von der Platte laden* / *Unterschied zeigen*; sonst still neu laden mit kurzem Hinweis).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant, SystemTime};

/// Abstand zwischen zwei Abfragen.
pub const POLL_INTERVAL: Duration = Duration::from_millis(1000);
/// Abstand für Dateien in WSL (#392): seltener, damit die WSL-VM ruhen kann.
pub const WSL_POLL_INTERVAL: Duration = Duration::from_secs(10);

/// Liegt die Datei in WSL (`\\wsl.localhost\…`)?
fn is_wsl(p: &Path) -> bool {
    crate::wsl::is_wsl_unc(&p.to_string_lossy())
}

/// Was über eine Datei bekannt ist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stamp {
    /// Datei fehlt (gelöscht, umbenannt) oder ist nicht lesbar.
    Missing,
    /// Größe und Änderungszeit.
    Present { len: u64, modified: Option<SystemTime> },
}

impl Stamp {
    /// Stand einer Datei jetzt (blockierend – im Hintergrund oder direkt nach dem eigenen Speichern aufrufen).
    pub fn of(path: &Path) -> Stamp {
        match std::fs::metadata(path) {
            Ok(m) if m.is_file() => Stamp::Present { len: m.len(), modified: m.modified().ok() },
            _ => Stamp::Missing,
        }
    }
}

/// Meldung an den Editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiskEvent {
    /// Inhalt hat sich geändert (Größe oder Zeit anders, Datei vorhanden).
    Changed(PathBuf),
    /// Datei ist verschwunden (gelöscht oder umbenannt).
    Deleted(PathBuf),
    /// Eine verschwundene Datei ist wieder da (z. B. von einem Werkzeug neu geschrieben).
    Restored(PathBuf),
}

impl DiskEvent {
    pub fn path(&self) -> &Path {
        match self {
            DiskEvent::Changed(p) | DiskEvent::Deleted(p) | DiskEvent::Restored(p) => p,
        }
    }
}

/// Stand je Pfad: bestätigt (`known`) und zuletzt gesehen, aber noch nicht stabil (`pending`).
#[derive(Clone, Copy, Debug)]
struct Track {
    known: Option<Stamp>,
    pending: Option<Stamp>,
}

/// Reine Vergleichslogik (getrennt vom Thread, damit sie testbar ist).
#[derive(Default, Debug)]
pub struct Tracker {
    files: HashMap<PathBuf, Track>,
}

impl Tracker {
    /// Eigener Stand nach Laden/Speichern: gilt sofort als bekannt, nichts wird gemeldet.
    pub fn baseline(&mut self, path: &Path, stamp: Stamp) {
        self.files.insert(path.to_path_buf(), Track { known: Some(stamp), pending: None });
    }

    /// Stand vergessen: die nächste Messung gilt wieder als Ausgangsstand (ohne Meldung).
    pub fn forget(&mut self, path: &Path) {
        self.files.remove(path);
    }

    /// Pfade, die nicht mehr offen sind, vergessen.
    pub fn retain(&mut self, open: &[PathBuf]) {
        self.files.retain(|p, _| open.iter().any(|o| o == p));
    }

    /// Messungen einer Abfrage einarbeiten; liefert die stabilen Änderungen.
    pub fn observe(&mut self, seen: &[(PathBuf, Stamp)]) -> Vec<DiskEvent> {
        let mut out = Vec::new();
        for (path, now) in seen {
            let t = self.files.entry(path.clone()).or_insert(Track { known: None, pending: None });
            let Some(known) = t.known else {
                // Erste Messung (Datei war beim Öffnen nicht als Stand übergeben): gilt als Ausgangsstand.
                t.known = Some(*now);
                continue;
            };
            if *now == known {
                t.pending = None;
                continue;
            }
            if t.pending != Some(*now) {
                // Neu oder noch in Bewegung: erst bei der nächsten gleichen Messung melden.
                t.pending = Some(*now);
                continue;
            }
            t.known = Some(*now);
            t.pending = None;
            out.push(match (known, *now) {
                (_, Stamp::Missing) => DiskEvent::Deleted(path.clone()),
                (Stamp::Missing, _) => DiskEvent::Restored(path.clone()),
                _ => DiskEvent::Changed(path.clone()),
            });
        }
        out
    }

    /// Ist der Pfad bekannt (für Tests)?
    #[cfg(test)]
    pub fn known(&self, path: &Path) -> Option<Stamp> {
        self.files.get(path).and_then(|t| t.known)
    }
}

/// Abfrage im Hintergrund plus [`Tracker`].
pub struct DiskWatch {
    tracker: Tracker,
    tx: Option<Sender<Vec<PathBuf>>>,
    rx: Option<Receiver<Vec<(PathBuf, Stamp)>>>,
    last: Option<Instant>,
    /// Letzte Abfrage der WSL-Dateien (seltener, [`WSL_POLL_INTERVAL`]).
    last_wsl: Option<Instant>,
    in_flight: bool,
}

impl Default for DiskWatch {
    fn default() -> Self {
        DiskWatch::new()
    }
}

impl DiskWatch {
    /// Der Thread startet erst bei der ersten Abfrage mit offenen Dateien.
    pub fn new() -> DiskWatch {
        DiskWatch { tracker: Tracker::default(), tx: None, rx: None, last: None, last_wsl: None, in_flight: false }
    }

    /// Nach dem eigenen Laden oder Speichern aufrufen (misst sofort; die Datei ist lokal gerade geschrieben). WSL-Dateien
    /// misst nie der UI-Thread (#392): Die nächste Abfrage im Hintergrund gilt als Ausgangsstand.
    pub fn baseline(&mut self, path: &Path) {
        if is_wsl(path) {
            self.tracker.forget(path);
            return;
        }
        self.tracker.baseline(path, Stamp::of(path));
    }

    fn spawn(&mut self, wake: Box<dyn Fn() + Send>) {
        let (tx, rx_paths) = std::sync::mpsc::channel::<Vec<PathBuf>>();
        let (tx_res, rx) = std::sync::mpsc::channel();
        let _ = std::thread::Builder::new().name("editor-disk-watch".into()).spawn(move || {
            // Endet, sobald `DiskWatch` (und damit der Sender) weg ist.
            while let Ok(paths) = rx_paths.recv() {
                let seen: Vec<(PathBuf, Stamp)> = paths.into_iter().map(|p| {
                    let s = Stamp::of(&p);
                    (p, s)
                }).collect();
                if tx_res.send(seen).is_err() {
                    break;
                }
                wake();
            }
        });
        self.tx = Some(tx);
        self.rx = Some(rx);
    }

    /// Je Bild aufrufen mit den Pfaden aller offenen Dokumente (ohne unbenannte). `wake` weckt die Oberfläche, wenn
    /// Messungen da sind (z. B. `ctx.request_repaint()`); `now` für Tests. Liefert die stabilen Änderungen.
    pub fn tick(&mut self, now: Instant, open: &[PathBuf], wake: impl Fn() + Send + 'static) -> Vec<DiskEvent> {
        let mut events = Vec::new();
        if let Some(rx) = &self.rx {
            while let Ok(seen) = rx.try_recv() {
                self.in_flight = false;
                // Inzwischen geschlossene Dateien nicht mehr melden.
                let seen: Vec<(PathBuf, Stamp)> = seen.into_iter().filter(|(p, _)| open.iter().any(|o| o == p)).collect();
                events.extend(self.tracker.observe(&seen));
            }
        }
        self.tracker.retain(open);
        if open.is_empty() || self.in_flight || self.last.is_some_and(|l| now.duration_since(l) < POLL_INTERVAL) {
            return events;
        }
        if self.tx.is_none() {
            self.spawn(Box::new(wake));
        }
        // WSL-Dateien nur alle WSL_POLL_INTERVAL (#392), die übrigen wie bisher.
        let wsl_due = self.last_wsl.is_none_or(|l| now.duration_since(l) >= WSL_POLL_INTERVAL);
        let list: Vec<PathBuf> = open.iter().filter(|p| wsl_due || !is_wsl(p)).cloned().collect();
        if wsl_due && open.iter().any(|p| is_wsl(p)) {
            self.last_wsl = Some(now);
        }
        if list.is_empty() {
            self.last = Some(now);
            return events;
        }
        if let Some(tx) = &self.tx
            && tx.send(list).is_ok()
        {
            self.in_flight = true;
            self.last = Some(now);
        }
        events
    }

    /// Wann die nächste Abfrage fällig ist (für `request_repaint_after`), `None` ohne offene Dateien.
    pub fn next_due(&self, now: Instant, any_open: bool) -> Option<Duration> {
        if !any_open {
            return None;
        }
        Some(self.last.map(|l| POLL_INTERVAL.saturating_sub(now.duration_since(l))).unwrap_or(Duration::ZERO))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn st(len: u64) -> Stamp {
        Stamp::Present { len, modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(len)) }
    }

    /// #392: WSL-Dateien misst nie der UI-Thread (Ausgangsstand = nächste Messung) und seltener.
    #[test]
    fn wsl_files_are_polled_less_often_and_never_measured_inline() {
        let w = p(r"\\wsl.localhost\U\home\u\a.txt");
        let mut dw = DiskWatch::new();
        dw.tracker.baseline(&w, st(1));
        dw.baseline(&w);
        assert_eq!(dw.tracker.known(&w), None, "vergessen statt gemessen");
        // Erste Messung danach gilt als Ausgangsstand, ohne Meldung
        assert!(dw.tracker.observe(&[(w.clone(), st(5))]).is_empty());
        assert_eq!(dw.tracker.known(&w), Some(st(5)));
        // Abfragen: die lokale Datei jedes Mal, die WSL-Datei erst nach WSL_POLL_INTERVAL
        let local = std::env::temp_dir().join(format!("pd-watch-{}.txt", std::process::id()));
        let t0 = Instant::now();
        let open = vec![local.clone(), w.clone()];
        let _ = dw.tick(t0, &open, || {});
        assert_eq!(dw.last_wsl, Some(t0), "erste Abfrage mit WSL");
        dw.in_flight = false;
        let _ = dw.tick(t0 + POLL_INTERVAL, &open, || {});
        assert_eq!(dw.last_wsl, Some(t0), "nach 1 s ohne WSL");
        dw.in_flight = false;
        let _ = dw.tick(t0 + WSL_POLL_INTERVAL, &open, || {});
        assert_eq!(dw.last_wsl, Some(t0 + WSL_POLL_INTERVAL));
    }

    #[test]
    fn changes_are_reported_once_and_only_when_stable() {
        let mut t = Tracker::default();
        t.baseline(&p("a"), st(1));
        assert!(t.observe(&[(p("a"), st(1))]).is_empty(), "unverändert");
        assert!(t.observe(&[(p("a"), st(2))]).is_empty(), "erste abweichende Messung wartet");
        assert!(t.observe(&[(p("a"), st(3))]).is_empty(), "noch in Bewegung");
        assert_eq!(t.observe(&[(p("a"), st(3))]), [DiskEvent::Changed(p("a"))]);
        assert!(t.observe(&[(p("a"), st(3))]).is_empty(), "nur einmal");
        // Kurz anders, dann wieder wie bekannt: nichts.
        assert!(t.observe(&[(p("a"), st(9))]).is_empty());
        assert!(t.observe(&[(p("a"), st(3))]).is_empty());
        assert!(t.observe(&[(p("a"), st(3))]).is_empty());
    }

    #[test]
    fn delete_and_restore() {
        let mut t = Tracker::default();
        t.baseline(&p("a"), st(1));
        t.observe(&[(p("a"), Stamp::Missing)]);
        assert_eq!(t.observe(&[(p("a"), Stamp::Missing)]), [DiskEvent::Deleted(p("a"))]);
        t.observe(&[(p("a"), st(5))]);
        assert_eq!(t.observe(&[(p("a"), st(5))]), [DiskEvent::Restored(p("a"))]);
    }

    #[test]
    fn own_save_is_not_a_change_and_unknown_files_start_quietly() {
        let mut t = Tracker::default();
        // Erste Messung ohne Ausgangsstand: wird zum Ausgangsstand.
        assert!(t.observe(&[(p("b"), st(4))]).is_empty());
        assert_eq!(t.known(&p("b")), Some(st(4)));
        // Eigenes Speichern: neuer Ausgangsstand, keine Meldung.
        t.observe(&[(p("b"), st(5))]);
        t.baseline(&p("b"), st(5));
        assert!(t.observe(&[(p("b"), st(5))]).is_empty());
        assert!(t.observe(&[(p("b"), st(5))]).is_empty());
        t.retain(&[]);
        assert_eq!(t.known(&p("b")), None);
    }

    #[test]
    fn the_background_watch_sees_a_real_change() {
        let dir = std::env::temp_dir().join(format!("pd-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("w.txt");
        std::fs::write(&f, "eins").unwrap();
        let mut w = DiskWatch::new();
        w.baseline(&f);
        let open = vec![f.clone()];
        std::fs::write(&f, "zwei, länger").unwrap();
        // Mehrere Abfragen mit künstlich fortschreitender Zeit; der Thread antwortet in der Regel sofort.
        let mut got = Vec::new();
        let t0 = Instant::now();
        for i in 0..200u64 {
            got.extend(w.tick(t0 + POLL_INTERVAL * (i as u32 + 1), &open, || {}));
            if !got.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(got, [DiskEvent::Changed(f.clone())]);
        assert!(w.next_due(t0, false).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
