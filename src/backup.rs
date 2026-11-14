//! Sicherungen der Datenbank: automatisch (Intervall, Nachholen beim Start), manuell, vor Migrationen und vor Wiederherstellungen.
//!
//! Eine Sicherung ist eine vollständige Kopie der Datenbankdatei (`VACUUM INTO`, konsistent auch bei laufendem Betrieb).
//! Dateinamen: `<art>-JJJJMMTT-HHMMSS.db` in lokaler Zeit. Nur automatische Sicherungen zählen gegen die einstellbare Anzahl;
//! manuelle bleiben, bis man sie löscht; Sicherungen vor Migration/Wiederherstellung werden auf die letzten drei begrenzt.
//! Wiederherstellen: wird als Marker (`restore.pending`) vorgemerkt und beim nächsten Start vor dem Öffnen der Datenbank eingespielt.

use chrono::NaiveDateTime;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

const STAMP: &str = "%Y%m%d-%H%M%S";
const PENDING: &str = "restore.pending";
const KEEP_SAFETY: usize = 3;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Auto,
    Manual,
    PreMigration { from: u32, to: u32 },
    PreRestore,
}

impl Kind {
    fn prefix(self) -> String {
        match self {
            Kind::Auto => "auto".into(),
            Kind::Manual => "manual".into(),
            Kind::PreMigration { from, to } => format!("pre-migration-v{from}-v{to}"),
            Kind::PreRestore => "pre-restore".into(),
        }
    }

    pub fn label(self, lang: crate::state::Lang) -> String {
        match self {
            Kind::Auto => lang.t("Automatisch", "Automatic").into(),
            Kind::Manual => lang.t("Manuell", "Manual").into(),
            Kind::PreMigration { from, to } => format!("{} v{from} → v{to}", lang.t("Vor Update", "Before update")),
            Kind::PreRestore => lang.t("Vor Wiederherstellung", "Before restore").into(),
        }
    }

    fn parse(prefix: &str) -> Option<Kind> {
        match prefix {
            "auto" => Some(Kind::Auto),
            "manual" => Some(Kind::Manual),
            "pre-restore" => Some(Kind::PreRestore),
            p => {
                let rest = p.strip_prefix("pre-migration-v")?;
                let (from, to) = rest.split_once("-v")?;
                Some(Kind::PreMigration { from: from.parse().ok()?, to: to.parse().ok()? })
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct BackupInfo {
    pub path: PathBuf,
    pub kind: Kind,
    pub time: NaiveDateTime,
    pub size: u64,
}

/// Sicherungsordner: der gewählte Ordner oder `backups/` im Datenordner.
pub fn dir(data_dir: &Path, custom: &str) -> PathBuf {
    let c = custom.trim();
    if c.is_empty() { data_dir.join("backups") } else { PathBuf::from(c) }
}

fn parse_name(file: &str) -> Option<(Kind, NaiveDateTime)> {
    let parts: Vec<&str> = file.strip_suffix(".db")?.split('-').collect();
    let digits = |s: &str, n: usize| s.len() == n && s.chars().all(|c| c.is_ascii_digit());
    // <art>-JJJJMMTT-HHMMSS[-<n>]: Datum und Uhrzeit sind zwei aufeinanderfolgende Teile; danach höchstens ein Zähler
    let i = (1..parts.len().saturating_sub(1)).find(|&i| digits(parts[i], 8) && digits(parts[i + 1], 6))?;
    if parts[i + 2..].len() > 1 || parts[i + 2..].iter().any(|s| s.is_empty() || !s.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    let time = NaiveDateTime::parse_from_str(&format!("{}-{}", parts[i], parts[i + 1]), STAMP).ok()?;
    Some((Kind::parse(&parts[..i].join("-"))?, time))
}

/// Alle Sicherungen im Ordner, neueste zuerst.
pub fn list(dir: &Path) -> Vec<BackupInfo> {
    let mut v: Vec<BackupInfo> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let (kind, time) = parse_name(&name)?;
            let size = e.metadata().ok()?.len();
            Some(BackupInfo { path: e.path(), kind, time, size })
        })
        .collect();
    v.sort_by(|a, b| b.time.cmp(&a.time).then_with(|| b.path.cmp(&a.path)));
    v
}

/// Legt eine Sicherung an (blockierend; für die Oberfläche `spawn` benutzen).
pub fn create(db_path: &Path, dest_dir: &Path, kind: Kind) -> Result<BackupInfo, String> {
    std::fs::create_dir_all(dest_dir).map_err(|e| format!("{}: {e}", dest_dir.display()))?;
    let now = chrono::Local::now().naive_local();
    let base = format!("{}-{}", kind.prefix(), now.format(STAMP));
    let mut dest = dest_dir.join(format!("{base}.db"));
    let mut n = 2;
    while dest.exists() {
        dest = dest_dir.join(format!("{base}-{n}.db"));
        n += 1;
    }
    let conn = Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(|e| e.to_string())?;
    conn.busy_timeout(std::time::Duration::from_secs(10)).map_err(|e| e.to_string())?;
    conn.execute("VACUUM INTO ?1", [dest.to_string_lossy().as_ref()]).map_err(|e| e.to_string())?;
    let size = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
    Ok(BackupInfo { path: dest, kind, time: now, size })
}

/// Entfernt alte Sicherungen: automatische über `keep_auto` hinaus, Sicherheitskopien (vor Update/Wiederherstellung) über drei hinaus.
pub fn rotate(dir: &Path, keep_auto: usize) {
    let all = list(dir);
    let mut auto = 0;
    let mut pre_mig = 0;
    let mut pre_restore = 0;
    for b in all {
        let remove = match b.kind {
            Kind::Auto => {
                auto += 1;
                auto > keep_auto
            }
            Kind::PreMigration { .. } => {
                pre_mig += 1;
                pre_mig > KEEP_SAFETY
            }
            Kind::PreRestore => {
                pre_restore += 1;
                pre_restore > KEEP_SAFETY
            }
            Kind::Manual => false,
        };
        if remove {
            let _ = std::fs::remove_file(&b.path);
        }
    }
}

/// Ist eine automatische Sicherung fällig? `reference` = letzte automatische Sicherung oder, falls es keine gibt, die Erstellung der Datenbank.
/// Mehrere verpasste Intervalle ergeben genau eine Sicherung (es wird nur "fällig" gemeldet, nicht nachgezählt).
pub fn due(reference: Option<NaiveDateTime>, interval_hours: u32, now: NaiveDateTime) -> bool {
    if interval_hours == 0 {
        return false;
    }
    match reference {
        Some(r) => now.signed_duration_since(r) >= chrono::Duration::hours(interval_hours as i64),
        None => false,
    }
}

/// Zeitpunkt der letzten automatischen Sicherung im Ordner.
pub fn last_auto(dir: &Path) -> Option<NaiveDateTime> {
    list(dir).into_iter().find(|b| b.kind == Kind::Auto).map(|b| b.time)
}

/// Sicherung im Hintergrund anlegen; das Ergebnis kommt über den Kanal (danach wird bei `Kind::Auto` rotiert).
pub fn spawn(db_path: PathBuf, dest_dir: PathBuf, kind: Kind, keep_auto: usize) -> mpsc::Receiver<Result<BackupInfo, String>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let r = create(&db_path, &dest_dir, kind);
        if r.is_ok() {
            rotate(&dest_dir, keep_auto);
        }
        let _ = tx.send(r);
    });
    rx
}

/// Wiederherstellung vormerken (wird beim nächsten Start eingespielt).
pub fn schedule_restore(data_dir: &Path, backup: &Path) -> Result<(), String> {
    check_valid(backup)?;
    std::fs::write(data_dir.join(PENDING), backup.to_string_lossy().as_bytes()).map_err(|e| e.to_string())
}

/// Ist die Datei eine intakte SQLite-Datenbank?
fn check_valid(path: &Path) -> Result<(), String> {
    let c = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| e.to_string())?;
    let r: String = c.query_row("PRAGMA quick_check", [], |r| r.get(0)).map_err(|e| e.to_string())?;
    if r == "ok" { Ok(()) } else { Err(r) }
}

/// Spielt eine vorgemerkte Wiederherstellung ein (vor dem Öffnen der Datenbank aufrufen). `None`, wenn nichts vorgemerkt war.
/// Der aktuelle Stand wird vorher als Sicherung „pre-restore“ abgelegt.
pub fn apply_pending_restore(data_dir: &Path, db_file: &str) -> Option<Result<PathBuf, String>> {
    let marker = data_dir.join(PENDING);
    let text = std::fs::read_to_string(&marker).ok()?;
    let _ = std::fs::remove_file(&marker);
    let backup = PathBuf::from(text.trim());
    let run = || -> Result<PathBuf, String> {
        check_valid(&backup)?;
        let db = data_dir.join(db_file);
        if db.exists() {
            let dest = backup.parent().map(Path::to_path_buf).unwrap_or_else(|| data_dir.join("backups"));
            create(&db, &dest, Kind::PreRestore)?;
            rotate(&dest, usize::MAX);
        }
        // Erst neben die Datenbank kopieren (scheitert das, z. B. Datenträger voll, bleibt die alte Datenbank unberührt), dann tauschen.
        let staged = data_dir.join(format!("{db_file}.restoring"));
        if let Err(e) = std::fs::copy(&backup, &staged) {
            let _ = std::fs::remove_file(&staged);
            return Err(format!("{}: {e}", staged.display()));
        }
        for suffix in ["", "-wal", "-shm"] {
            let f = data_dir.join(format!("{db_file}{suffix}"));
            if f.exists() {
                if let Err(e) = std::fs::remove_file(&f) {
                    let _ = std::fs::remove_file(&staged);
                    return Err(format!("{}: {e}", f.display()));
                }
            }
        }
        std::fs::rename(&staged, &db).map_err(|e| e.to_string())?;
        Ok(backup.clone())
    };
    Some(run())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("paneldeck-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn at(h: u32, m: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 11, 14).unwrap().and_hms_opt(h, m, 0).unwrap()
    }

    fn make_db(path: &Path, marker: &str) {
        let c = Connection::open(path).unwrap();
        c.execute_batch(&format!("CREATE TABLE IF NOT EXISTS t (x TEXT); DELETE FROM t; INSERT INTO t VALUES ('{marker}');")).unwrap();
    }

    fn marker(path: &Path) -> String {
        Connection::open(path).unwrap().query_row("SELECT x FROM t", [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn file_names_round_trip() {
        let (k, t) = parse_name("auto-20261114-103005.db").unwrap();
        assert_eq!(k, Kind::Auto);
        assert_eq!(t, NaiveDate::from_ymd_opt(2026, 11, 14).unwrap().and_hms_opt(10, 30, 5).unwrap());
        assert_eq!(parse_name("manual-20261114-103005-2.db").unwrap().0, Kind::Manual);
        assert_eq!(parse_name("pre-migration-v1-v2-20261114-103005.db").unwrap().0, Kind::PreMigration { from: 1, to: 2 });
        assert_eq!(parse_name("pre-restore-20261114-103005.db").unwrap().0, Kind::PreRestore);
        assert!(parse_name("paneldeck.db").is_none());
        assert!(parse_name("auto-xyz.db").is_none());
    }

    #[test]
    fn due_logic_catches_up_with_a_single_backup() {
        // 24 h Intervall: nach 23 h nicht fällig, nach 24 h und nach 5 Tagen (Rechner war aus) fällig
        assert!(!due(Some(at(8, 0)), 24, at(8, 0) + chrono::Duration::hours(23)));
        assert!(due(Some(at(8, 0)), 24, at(8, 0) + chrono::Duration::hours(24)));
        assert!(due(Some(at(8, 0)), 24, at(8, 0) + chrono::Duration::days(5)));
        assert!(!due(Some(at(8, 0)), 0, at(8, 0) + chrono::Duration::days(5)), "Intervall 0 = aus");
        assert!(!due(None, 24, at(8, 0)));
    }

    #[test]
    fn create_list_rotate_keeps_manual_and_counts_only_auto() {
        let d = tmp("bk-rotate");
        let db = d.join("paneldeck.db");
        make_db(&db, "eins");
        let bdir = d.join("backups");
        for _ in 0..4 {
            create(&db, &bdir, Kind::Auto).unwrap();
        }
        create(&db, &bdir, Kind::Manual).unwrap();
        assert_eq!(list(&bdir).len(), 5);
        rotate(&bdir, 2);
        let l = list(&bdir);
        assert_eq!(l.iter().filter(|b| b.kind == Kind::Auto).count(), 2, "nur die zwei neuesten automatischen bleiben");
        assert_eq!(l.iter().filter(|b| b.kind == Kind::Manual).count(), 1, "manuelle werden nie rotiert");
        assert_eq!(marker(&l[0].path), "eins");
        assert!(last_auto(&bdir).is_some());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn restore_replaces_db_and_keeps_a_safety_copy() {
        let d = tmp("bk-restore");
        let db = d.join("paneldeck.db");
        make_db(&db, "alt");
        let bdir = d.join("backups");
        let b = create(&db, &bdir, Kind::Manual).unwrap();
        make_db(&db, "neu");
        schedule_restore(&d, &b.path).unwrap();
        let r = apply_pending_restore(&d, "paneldeck.db").expect("vorgemerkt").unwrap();
        assert_eq!(r, b.path);
        assert_eq!(marker(&db), "alt", "die Datenbank enthält wieder den gesicherten Stand");
        let pre: Vec<_> = list(&bdir).into_iter().filter(|x| x.kind == Kind::PreRestore).collect();
        assert_eq!(pre.len(), 1, "der Stand vor der Wiederherstellung wurde gesichert");
        assert_eq!(marker(&pre[0].path), "neu");
        assert!(apply_pending_restore(&d, "paneldeck.db").is_none(), "Marker ist verbraucht");
        assert!(!d.join("paneldeck.db.restoring").exists(), "die Zwischendatei wird umbenannt, nicht liegengelassen");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Kann die Sicherung nicht zur Wiederherstellung kopiert werden, bleibt die alte Datenbank unberührt (nichts wird vorher gelöscht).
    #[test]
    fn failed_restore_leaves_the_database_alone() {
        let d = tmp("bk-restore-fail");
        let db = d.join("paneldeck.db");
        make_db(&db, "jetzt");
        let bdir = d.join("backups");
        let b = create(&db, &bdir, Kind::Manual).unwrap();
        schedule_restore(&d, &b.path).unwrap();
        // Die Zwischendatei lässt sich nicht anlegen: an ihrer Stelle steht ein Ordner.
        std::fs::create_dir_all(d.join("paneldeck.db.restoring")).unwrap();
        make_db(&db, "neuer");
        let r = apply_pending_restore(&d, "paneldeck.db").expect("vorgemerkt");
        assert!(r.is_err());
        assert_eq!(marker(&db), "neuer", "die Datenbank ist unverändert");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn restore_rejects_a_corrupt_backup() {
        let d = tmp("bk-corrupt");
        let bad = d.join("auto-20261114-103005.db");
        std::fs::write(&bad, b"das ist keine datenbank").unwrap();
        assert!(schedule_restore(&d, &bad).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn background_backup_reports_back() {
        let d = tmp("bk-spawn");
        let db = d.join("paneldeck.db");
        make_db(&db, "x");
        let rx = spawn(db, d.join("backups"), Kind::Auto, 2);
        let r = rx.recv_timeout(std::time::Duration::from_secs(10)).expect("Antwort").unwrap();
        assert!(r.path.exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
