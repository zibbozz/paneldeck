//! Tests der Datenbankpflege: Reparatur eindeutiger Fälle, Zähler, Verdichten, Statistik und die Rettung einer absichtlich
//! beschädigten Datenbankdatei in eine neue Datei.

use super::dbcare::*;
use super::fixture::build;
use super::DbScope;
use crate::planning::history::Source;
use crate::state::Lang;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("paneldeck-maint-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn repair_fixes_only_unambiguous_cases_and_counters() {
    let m = build();
    let c = &m.fx.c;
    let plan = plan_repair(c).unwrap();
    assert!(plan.iter().any(|s| matches!(s, RepairStep::Orphans { r, fix: OrphanFix::Delete, .. } if r.table == "comments")), "{plan:?}");
    assert!(plan.iter().any(|s| matches!(s, RepairStep::Orphans { r, fix: OrphanFix::SetNull, .. } if r.table == "tickets" && r.column == "parent_id")));
    assert!(plan.iter().any(|s| matches!(s, RepairStep::Counter(Counter::ProjectNumber { needed: 15, .. }))));
    assert!(plan.iter().all(|s| !s.text(Lang::De).is_empty() && !s.text(Lang::En).is_empty()));
    let n = apply_repair(c, &plan).unwrap();
    assert!(n >= 3);
    assert!(orphans(c).unwrap().is_empty());
    assert!(counters(c).unwrap().is_empty());
    let next: i64 = c.query_row("SELECT next_number FROM projects WHERE key = 'PAN'", [], |r| r.get(0)).unwrap();
    assert_eq!(next, 15);
    // ein neues Ticket bekommt keine doppelte Nummer
    let t = m.fx.mk(m.fx.defect, "after repair");
    assert_eq!(t.key(), "PAN-15");
    assert!(plan_repair(c).unwrap().is_empty());
}

#[test]
fn ambiguous_orphans_are_only_reported() {
    let m = build();
    let c = &m.fx.c;
    c.pragma_update(None, "foreign_keys", false).unwrap();
    c.execute("UPDATE tickets SET type_id = 4242 WHERE id = ?1", [m.id(3)]).unwrap();
    c.pragma_update(None, "foreign_keys", true).unwrap();
    let plan = plan_repair(c).unwrap();
    let step = plan.iter().find(|s| matches!(s, RepairStep::Orphans { r, .. } if r.table == "tickets" && r.column == "type_id")).unwrap();
    assert!(!step.applies());
    apply_repair(c, &plan).unwrap();
    let still: i64 = c.query_row("SELECT type_id FROM tickets WHERE id = ?1", [m.id(3)], |r| r.get(0)).unwrap();
    assert_eq!(still, 4242, "nicht eindeutig: bleibt unverändert");
}

#[test]
fn repair_never_deletes_tickets_of_a_missing_project() {
    let m = build();
    let c = &m.fx.c;
    // Projekt der Tickets verschwindet (ohne Kaskade): Tickets verweisen auf ein fehlendes Projekt.
    c.pragma_update(None, "foreign_keys", false).unwrap();
    c.execute("UPDATE tickets SET project_id = 9999 WHERE id IN (?1, ?2)", [m.id(1), m.id(2)]).unwrap();
    let comments_before: i64 = c.query_row("SELECT COUNT(*) FROM comments", [], |r| r.get(0)).unwrap();
    c.pragma_update(None, "foreign_keys", true).unwrap();
    let r = FkRef { table: "tickets".into(), column: "project_id".into(), parent: "projects".into(), parent_column: "id".into() };
    assert_eq!(orphan_fix(c, &r), OrphanFix::Manual, "Tickets werden nie gelöscht");
    // auch Tabellen, deren Löschen in Ticketdaten weiterläuft (Felder → Werte, Typen → Felder → Werte)
    for (t, col, p) in [("fields", "type_id", "ticket_types"), ("ticket_types", "project_id", "projects"), ("sprints", "project_id", "projects")] {
        let r = FkRef { table: t.into(), column: col.into(), parent: p.into(), parent_column: "id".into() };
        assert_eq!(orphan_fix(c, &r), OrphanFix::Manual, "{t}");
    }
    // Kommentare ohne Ticket dürfen weiter gelöscht werden
    let r = FkRef { table: "comments".into(), column: "ticket_id".into(), parent: "tickets".into(), parent_column: "id".into() };
    assert_eq!(orphan_fix(c, &r), OrphanFix::Delete);
    let plan = plan_repair(c).unwrap();
    let step = plan.iter().find(|s| matches!(s, RepairStep::Orphans { r, .. } if r.table == "tickets" && r.column == "project_id")).unwrap();
    assert!(!step.applies());
    let findings = findings(c, DbScope::Quick, Lang::En).unwrap();
    let f = findings.iter().find(|f| f.id == "db_foreign_key:tickets.project_id").unwrap();
    assert!(f.fixes.is_empty(), "{f:?}");
    // Auch ein „alter“ Plan mit Delete für Tickets löscht nichts.
    let forged = vec![RepairStep::Orphans { r: FkRef { table: "tickets".into(), column: "project_id".into(), parent: "projects".into(), parent_column: "id".into() }, rows: 2, fix: OrphanFix::Delete }];
    apply_repair(c, &forged).unwrap();
    apply_repair(c, &plan).unwrap();
    let n: i64 = c.query_row("SELECT COUNT(*) FROM tickets WHERE id IN (?1, ?2)", [m.id(1), m.id(2)], |r| r.get(0)).unwrap();
    assert_eq!(n, 2, "die Tickets sind noch da");
    let comments_after: i64 = c.query_row("SELECT COUNT(*) FROM comments", [], |r| r.get(0)).unwrap();
    assert_eq!(comments_after, comments_before - 1, "nur der Kommentar ohne Ticket aus dem Testprojekt ist weg");
}

#[test]
fn repair_keeps_still_referenced_orphans_and_applies_the_rest() {
    let m = build();
    let c = &m.fx.c;
    // Ein Status verliert sein Projekt, wird aber noch von einem Ticket benutzt; ein zweiter verwaister Status ist unbenutzt.
    c.pragma_update(None, "foreign_keys", false).unwrap();
    c.execute("UPDATE statuses SET project_id = 9999 WHERE id = ?1", [m.fx.progress]).unwrap();
    c.execute("INSERT INTO statuses (project_id, name, category, color, position) SELECT 9999, 'Lost', 'todo', color, 99 FROM statuses WHERE id = ?1", [m.fx.progress]).unwrap();
    let lost = c.last_insert_rowid();
    c.pragma_update(None, "foreign_keys", true).unwrap();
    let used: i64 = c.query_row("SELECT COUNT(*) FROM tickets WHERE status_id = ?1", [m.fx.progress], |r| r.get(0)).unwrap();
    assert!(used > 0);
    let plan = plan_repair(c).unwrap();
    let st: Vec<&RepairStep> = plan.iter().filter(|s| matches!(s, RepairStep::Orphans { r, .. } if r.table == "statuses")).collect();
    assert!(st.iter().any(|s| matches!(s, RepairStep::Orphans { rows: 1, fix: OrphanFix::Delete, .. })), "{st:?}");
    assert!(st.iter().any(|s| matches!(s, RepairStep::Orphans { rows: 1, fix: OrphanFix::Manual, .. })), "{st:?}");
    // die Reparatur gelingt mit foreign_keys = ON und wendet auch die übrigen eindeutigen Schritte an
    apply_repair(c, &plan).unwrap();
    let still: i64 = c.query_row("SELECT COUNT(*) FROM statuses WHERE id = ?1", [m.fx.progress], |r| r.get(0)).unwrap();
    assert_eq!(still, 1, "noch benutzter Status bleibt");
    let gone: i64 = c.query_row("SELECT COUNT(*) FROM statuses WHERE id = ?1", [lost], |r| r.get(0)).unwrap();
    assert_eq!(gone, 0, "unbenutzter verwaister Status ist weg");
    assert!(counters(c).unwrap().is_empty(), "Zähler sind trotzdem repariert");
    let orphan_comments: i64 = c.query_row("SELECT COUNT(*) FROM comments WHERE ticket_id NOT IN (SELECT id FROM tickets)", [], |r| r.get(0)).unwrap();
    assert_eq!(orphan_comments, 0);
}

#[test]
fn statistics_per_project_and_whole_database() {
    let m = build();
    let p = stats(&m.fx.c, Some(m.fx.project)).unwrap();
    assert_eq!(p.tickets, 14);
    assert_eq!(p.archived, 1);
    assert!(p.links >= 2 && p.history > 0 && p.file_bytes > 0);
    assert!(!p.largest.is_empty() && !p.growth.is_empty());
    let all = stats(&m.fx.c, None).unwrap();
    assert!(all.tickets >= p.tickets);
    assert_eq!(human_bytes(2048), "2 KB");
}

/// Datenbankdatei mit Projekt, vielen Tickets und großen Kommentaren (mehrere Seiten), danach geschlossen.
fn make_file_db(dir: &Path) -> (PathBuf, i64, i64) {
    let db = crate::db::open(dir, &dir.join("backups")).unwrap();
    let c = &db.conn;
    let mut np = crate::planning::projects::NewProject::new("RES", "Rescue", "#112233");
    np.with_defaults = true;
    let p = crate::planning::projects::create_project(c, &np).unwrap();
    let ty = crate::planning::types::list_types(c, p).unwrap().into_iter().find(|t| t.key.as_deref() == Some("devtask")).unwrap().id;
    let mut ids = Vec::new();
    for i in 0..40 {
        ids.push(crate::planning::tickets::create_ticket(c, &crate::planning::tickets::NewTicket::new(p, ty, &format!("Ticket {i}")), &Source::ui()).unwrap().0.id);
    }
    let body = "x".repeat(1500);
    for i in 0..400 {
        crate::planning::comments::add_comment(c, ids[i % ids.len()], &format!("{i} {body}"), &Source::ui()).unwrap();
    }
    let tickets: i64 = c.query_row("SELECT COUNT(*) FROM tickets", [], |r| r.get(0)).unwrap();
    let comments: i64 = c.query_row("SELECT COUNT(*) FROM comments", [], |r| r.get(0)).unwrap();
    let path = db.path.clone();
    drop(db);
    // WAL in die Datei übernehmen
    let c = Connection::open(&path).unwrap();
    c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode = DELETE;").unwrap();
    drop(c);
    (path, tickets, comments)
}

#[test]
fn compact_shows_before_and_after() {
    let d = tmp("compact");
    let (path, _, _) = make_file_db(&d);
    let c = Connection::open(&path).unwrap();
    c.execute("DELETE FROM comments", []).unwrap();
    drop(c);
    let r = compact(&path).unwrap();
    assert!(r.after < r.before, "{r:?}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn rescue_of_a_damaged_database_writes_a_new_file_and_a_report() {
    let d = tmp("rescue");
    let (path, tickets, comments) = make_file_db(&d);
    // absichtlich beschädigen: einige Seiten nahe dem Dateiende (Kommentar-Seiten) überschreiben
    let mut bytes = std::fs::read(&path).unwrap();
    let page = 4096;
    let pages = bytes.len() / page;
    for p in (pages - 12)..(pages - 8) {
        for b in &mut bytes[p * page..(p + 1) * page] {
            *b = 0x5a;
        }
    }
    std::fs::write(&path, &bytes).unwrap();
    let before = std::fs::read(&path).unwrap();
    {
        let c = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let problems = integrity(&c, DbScope::Full).unwrap_or_else(|e| vec![e.to_string()]);
        assert!(!problems.is_empty(), "die Datei ist beschädigt");
        assert!(explain_integrity(&problems[0], Lang::En).contains('('));
    }
    let out = d.join("rescued");
    let rep = rescue(&path, &out, Lang::En).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), before, "die beschädigte Datei bleibt unverändert");
    assert_ne!(rep.dest, path);
    assert!(rep.dest.exists() && rep.dest.with_extension("txt").exists());
    assert!(rep.dest_problems.is_empty(), "{:?}", rep.dest_problems);
    let c = Connection::open(&rep.dest).unwrap();
    let t: i64 = c.query_row("SELECT COUNT(*) FROM tickets", [], |r| r.get(0)).unwrap();
    let k: i64 = c.query_row("SELECT COUNT(*) FROM comments", [], |r| r.get(0)).unwrap();
    assert_eq!(t, tickets, "Tickets sind vollständig gerettet");
    assert!(k <= comments && k >= comments / 2, "{k} von {comments}");
    let v: u32 = c.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
    assert_eq!(v, crate::db::schema_version());
    let projects: i64 = c.query_row("SELECT COUNT(*) FROM projects WHERE key IN ('TODO', 'RES')", [], |r| r.get(0)).unwrap();
    assert_eq!(projects, 2, "keine doppelten Vorgaben des neuen Schemas");
    let text = rep.text(Lang::En);
    assert!(text.contains("tickets:") && text.contains("rescued"), "{text}");
    assert!(rep.rescued() > 0);
    drop(c);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn rescue_of_an_intact_database_is_complete() {
    let d = tmp("rescue-ok");
    let (path, tickets, comments) = make_file_db(&d);
    let rep = rescue(&path, &d.join("r"), Lang::De).unwrap();
    assert!(rep.complete(), "{rep:?}");
    let c = Connection::open(&rep.dest).unwrap();
    assert_eq!(c.query_row("SELECT COUNT(*) FROM tickets", [], |r| r.get::<_, i64>(0)).unwrap(), tickets);
    assert_eq!(c.query_row("SELECT COUNT(*) FROM comments", [], |r| r.get::<_, i64>(0)).unwrap(), comments);
    assert!(rep.text(Lang::De).contains("gerettet"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn rescue_counts_rows_with_unreadable_columns_as_partial_and_logs_only_into_the_new_file() {
    let d = tmp("rescue-partial");
    let (path, _, comments) = make_file_db(&d);
    let bad_id: i64 = {
        let c = Connection::open(&path).unwrap();
        // Text mit ungültigem UTF-8 (lässt sich nicht als Text lesen)
        c.execute("UPDATE comments SET body = CAST(x'4142ff43' AS TEXT) WHERE id = (SELECT MIN(id) FROM comments)", []).unwrap();
        c.query_row("SELECT MIN(id) FROM comments", [], |r| r.get(0)).unwrap()
    };
    let log_before: i64 = Connection::open(&path).unwrap().query_row("SELECT COUNT(*) FROM maintenance_log", [], |r| r.get(0)).unwrap();
    let before = std::fs::read(&path).unwrap();
    let rep = rescue(&path, &d.join("r"), Lang::En).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), before, "Quelle unverändert (auch kein Protokolleintrag)");
    let t = rep.tables.iter().find(|t| t.table == "comments").unwrap();
    assert_eq!(t.partial, 1, "{t:?}");
    assert_eq!(t.rescued, comments - 1);
    assert_eq!(t.lost(), Some(0));
    assert_eq!(t.partial_rows, vec![format!("rowid {bad_id}")]);
    assert!(!rep.complete());
    assert_eq!(rep.partial(), 1);
    let text = rep.text(Lang::En);
    assert!(text.contains("1 only partly") && text.contains(&format!("comments: rowid {bad_id}")), "{text}");
    assert!(rep.text(Lang::De).contains("teilweise"));
    let c = Connection::open(&rep.dest).unwrap();
    let body: String = c.query_row("SELECT body FROM comments WHERE id = ?1", [bad_id], |r| r.get(0)).unwrap();
    assert_eq!(body, "AB\u{fffd}C", "unlesbarer Text mit Ersatzzeichen übernommen");
    let logged: i64 = c.query_row("SELECT COUNT(*) FROM maintenance_log WHERE action = 'rescue'", [], |r| r.get(0)).unwrap();
    assert_eq!(logged, 1, "Protokoll in der neuen Datei");
    let log_after: i64 = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap().query_row("SELECT COUNT(*) FROM maintenance_log", [], |r| r.get(0)).unwrap();
    assert_eq!(log_after, log_before);
    drop(c);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn rescue_report_never_promises_to_delete_tickets() {
    let rep = RescueReport { source: "a.db".into(), dest: "b.db".into(), tables: vec![], source_version: None, dest_problems: vec![], dest_orphans: 3 };
    for lang in [Lang::De, Lang::En] {
        let text = rep.text(lang);
        assert!(text.contains(lang.t("löscht sie nie", "never deletes tickets")), "{text}");
        assert!(!text.contains("cleans them up") && !text.contains("räumt sie auf"));
    }
}
