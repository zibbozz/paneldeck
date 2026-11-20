//! Erzeugtes Testprojekt mit bekannten Problemen für die Wartung (Unit-Tests; dieselben Probleme legt `tools/smoke-mcp.ps1`
//! per Python für die Ende-zu-Ende-Prüfung von `maintenance_report` an).
//!
//! Projekt `PAN` (Standard-Typen), „jetzt“ = 2026-11-20 12:00. Bekannte Befunde:
//! * PAN-1 nie begonnen, 200 Tage unverändert → `stale_new`
//! * PAN-2 in Arbeit, 120 Tage unverändert → `stale_in_progress`
//! * PAN-3 frisch, Verknüpfung auf ein gelöschtes Ticket → `broken_reference`
//! * PAN-4 überfällig seit 2026-10-01 → `overdue`
//! * PAN-5 / PAN-6 fast gleiche Titel → `possible_duplicate`
//! * PAN-7 (Epic, erledigt) mit offenem Kind PAN-8 → `parent_done_children_open`
//! * PAN-9 Eltern-Ticket gelöscht → `orphaned_subtask`
//! * PAN-10 blockiert PAN-11 blockiert PAN-10 → `link_cycle`
//! * PAN-12 Pflichtfeld „Area“ leer → `missing_required`
//! * PAN-13 Priorität mit gelöschtem Auswahlwert → `invalid_value`
//! * Labels `bug` (2×) und `Bug` (1×) → `labels_similar`
//! * PAN-14 archiviert und uralt → taucht nirgends auf
//! * Zähler `next_number` zu klein, Kommentar ohne Ticket → `db_counter`, `db_foreign_key`

use crate::planning::fields::FieldKind;
use crate::planning::history::Source;
use crate::planning::link_types::link_type_by_key;
use crate::planning::links::{add_link, Target};
use crate::planning::tickets::fixture::Fx;
use crate::planning::tickets::{create_child, set_field, Ticket};
use crate::planning::transition::transition_ticket;
use chrono::{NaiveDate, NaiveDateTime};
use serde_json::{json, Value};

pub struct MaintFx {
    pub fx: Fx,
    pub now: NaiveDateTime,
    pub t: Vec<Ticket>,
}

impl MaintFx {
    pub fn key(&self, n: usize) -> String {
        format!("PAN-{n}")
    }

    pub fn id(&self, n: usize) -> i64 {
        self.t[n - 1].id
    }
}

pub fn now() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 11, 20).unwrap().and_hms_opt(12, 0, 0).unwrap()
}

pub fn build() -> MaintFx {
    let f = Fx::new();
    let ui = Source::ui();
    let mut t: Vec<Ticket> = Vec::new();
    t.push(f.mk(f.task, "Write the onboarding guide")); // 1
    t.push(f.mk(f.task, "Refactor the session store")); // 2
    transition_ticket(&f.c, t[1].id, f.progress, &[], &ui).unwrap();
    t.push(f.mk(f.task, "Fresh work item")); // 3
    t.push(f.mk(f.task, "Ship the release notes")); // 4
    let due = f.field(f.task, crate::planning::builtin_fields::DUE_KEY);
    set_field(&f.c, t[3].id, due.id, json!("2026-10-01"), &ui).unwrap();
    t.push(f.mk(f.defect, "Login fails on Safari")); // 5
    t.push(f.mk(f.defect, "Login fails in Safari")); // 6
    let epic = f.mk(f.epic, "Checkout epic"); // 7
    t.push(epic.clone());
    t.push(create_child(&f.c, epic.id, f.story, "Pay with card", vec![], &ui).unwrap().0); // 8
    // Epic erledigt (Kind bleibt offen); Status direkt, die Regeln sind hier nicht das Thema
    f.c.execute("UPDATE tickets SET status_id = ?1 WHERE id = ?2", [f.done, epic.id]).unwrap();
    t.push(f.mk(f.task, "Child of a deleted parent")); // 9
    t.push(f.mk(f.task, "Blocked by the importer")); // 10
    t.push(f.mk(f.task, "Waiting for deployment rights")); // 11
    let blocks = link_type_by_key(&f.c, "blocks").unwrap().id;
    add_link(&f.c, t[9].id, blocks, Target::Ticket(t[10].id), &ui).unwrap();
    add_link(&f.c, t[10].id, blocks, Target::Ticket(t[9].id), &ui).unwrap();
    t.push(f.mk(f.task, "Missing area")); // 12
    t.push(f.mk(f.task, "Bad priority")); // 13
    t.push(f.mk(f.task, "Very old archived")); // 14
    // Labels
    let labels = f.field(f.task, crate::planning::builtin_fields::LABELS_KEY);
    set_field(&f.c, t[0].id, labels.id, json!(["bug"]), &ui).unwrap();
    set_field(&f.c, t[2].id, labels.id, json!(["bug", "ui"]), &ui).unwrap();
    set_field(&f.c, t[3].id, labels.id, json!(["Bug"]), &ui).unwrap();
    // Pflichtfeld nachträglich (PAN-12 hat keinen Wert; alle anderen Aufgaben bekommen einen)
    let area = f.add_field(f.task, "Area", FieldKind::Text, Default::default(), false);
    for (i, tk) in t.iter().enumerate() {
        if tk.type_id == f.task && i != 11 {
            f.c.execute("INSERT INTO ticket_values (ticket_id, field_id, pos, text) VALUES (?1, ?2, 0, 'core')", [tk.id, area.id]).unwrap();
        }
    }
    f.c.execute("UPDATE fields SET required = 1 WHERE id = ?1", [area.id]).unwrap();
    // gelöschter Auswahlwert in der Priorität von PAN-13
    let prio = f.field(f.task, "priority");
    f.c.execute("INSERT OR REPLACE INTO ticket_values (ticket_id, field_id, pos, num) VALUES (?1, ?2, 0, 99999)", [t[12].id, prio.id]).unwrap();
    // Kaputtes per Hand (wie nach einem Bearbeiten mit abgeschalteten Fremdschlüsseln)
    f.c.pragma_update(None, "foreign_keys", false).unwrap();
    f.c.execute("UPDATE tickets SET parent_id = 9999 WHERE id = ?1", [t[8].id]).unwrap();
    f.c.execute("INSERT INTO links (link_type_id, from_ticket_id, to_kind, to_ticket_id, to_text, created_at) VALUES (?1, ?2, 'ticket', 9998, NULL, '2026-11-01T00:00:00')", [blocks, t[2].id]).unwrap();
    f.c.execute("INSERT INTO comments (ticket_id, body, created_at, updated_at) VALUES (9997, 'lost', '2026-11-01T00:00:00', '2026-11-01T00:00:00')", []).unwrap();
    f.c.pragma_update(None, "foreign_keys", true).unwrap();
    f.c.execute("UPDATE projects SET next_number = 3 WHERE id = ?1", [f.project]).unwrap();
    // Zeiten: alles frisch, außer PAN-1 (200 Tage), PAN-2 (120 Tage), PAN-14 (archiviert, 400 Tage)
    f.c.execute("UPDATE tickets SET updated_at = '2026-11-15T10:00:00' WHERE project_id = ?1", [f.project]).unwrap();
    f.c.execute("UPDATE tickets SET updated_at = '2026-05-04T10:00:00' WHERE id = ?1", [t[0].id]).unwrap();
    f.c.execute("UPDATE tickets SET updated_at = '2026-07-23T10:00:00' WHERE id = ?1", [t[1].id]).unwrap();
    f.c.execute("UPDATE tickets SET updated_at = '2025-10-15T10:00:00', archived = 1 WHERE id = ?1", [t[13].id]).unwrap();
    let t = t.iter().map(|x| crate::planning::tickets::get_ticket(&f.c, x.id).unwrap()).collect();
    let _ = Value::Null;
    MaintFx { fx: f, now: now(), t }
}
