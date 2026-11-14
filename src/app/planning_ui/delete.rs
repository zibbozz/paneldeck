//! Löschen von Status, Typen, Feldern, Verknüpfungstypen und Arten der Arbeit. Wird der Eintrag noch benutzt, führt der
//! Dialog zu „Ersatz wählen oder verwerfen“ (mit Zähler); bei Typen mit Zuordnung der Felder (`suggest_field_map`).

use super::widgets;
use super::{Cx, PlanUi};
use crate::planning::fields::{self, FieldKind};
use crate::planning::integrity::{DeleteMode, FieldMapEntry, TypeDeleteMode, TypeMigration};
use crate::planning::{link_types, statuses, types, work_kinds, PlanError};
use crate::state::Lang;
use crate::style::{S1, S2};
use eframe::egui::{self, RichText};

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Target {
    Status(i64),
    Type(i64),
    Field(i64),
    LinkType(i64),
    WorkKind(i64),
}

pub(super) struct Dlg {
    target: Target,
    name: String,
    usage: i64,
    /// `true`: Verwendungen auf den Ersatz umhängen; `false`: verwerfen.
    replace: bool,
    to: Option<i64>,
    cands: Vec<(i64, String)>,
    /// Nur Typen: Zuordnung (altes Feld → Feld des Ersatz-Typs oder verwerfen).
    map: Vec<FieldMapEntry>,
    err: Option<String>,
}

/// Wörter je Art des Eintrags: (Nomen, wer benutzt es, was passiert beim Verwerfen, was beim Löschen ohne Benutzung).
fn words(t: Target, lang: Lang) -> (&'static str, &'static str, &'static str, &'static str) {
    match t {
        Target::Status(_) => (
            lang.t("Status", "status"),
            lang.t("Tickets haben diesen Status.", "ticket(s) have this status."),
            lang.t("Der Status dieser Tickets wird geleert.", "The status of these tickets is cleared."),
            lang.t("Der Status wird aus allen Workflows und Übergängen entfernt.", "The status is removed from all workflows and transitions."),
        ),
        Target::Type(_) => (
            lang.t("Typ", "type"),
            lang.t("Tickets sind von diesem Typ.", "ticket(s) are of this type."),
            lang.t("Diese Tickets werden mitgelöscht.", "These tickets are deleted too."),
            lang.t("Der Typ wird samt Feldern, Workflow und Hierarchie-Regeln gelöscht.", "The type is deleted with its fields, workflow and hierarchy rules."),
        ),
        Target::Field(_) => (
            lang.t("Feld", "field"),
            lang.t("gespeicherte Werte gibt es in diesem Feld.", "stored value(s) exist in this field."),
            lang.t("Diese Werte werden verworfen.", "These values are discarded."),
            lang.t("Das Feld wird gelöscht; Übergangsregeln, die darauf verweisen, entfallen.", "The field is deleted; transition rules that refer to it are removed."),
        ),
        Target::LinkType(_) => (
            lang.t("Verknüpfungstyp", "link type"),
            lang.t("Verknüpfungen haben diesen Typ.", "link(s) use this type."),
            lang.t("Diese Verknüpfungen werden gelöscht.", "These links are deleted."),
            lang.t("Der Verknüpfungstyp wird gelöscht.", "The link type is deleted."),
        ),
        Target::WorkKind(_) => (
            lang.t("Art der Arbeit", "kind of work"),
            lang.t("Zeiteinträge haben diese Art der Arbeit.", "time entries use this kind of work."),
            lang.t("Diese Zeiteinträge werden gelöscht.", "These time entries are deleted."),
            lang.t("Die Art der Arbeit wird aus der Liste entfernt.", "The kind of work is removed from the list."),
        ),
    }
}

impl PlanUi {
    /// Löschen vorbereiten: Name, Zähler und mögliche Ersätze bestimmen und den Dialog öffnen.
    pub(super) fn ask_delete(&mut self, cx: &Cx, target: Target) {
        let lang = cx.lang;
        let Some(sum) = self.sum.clone() else { return };
        let (name, usage, cands): (String, Result<i64, PlanError>, Vec<(i64, String)>) = match target {
            Target::Status(id) => (
                sum.status(id).map(|s| s.display(lang)).unwrap_or_default(),
                statuses::usage(cx.conn, id),
                sum.statuses.iter().filter(|s| s.id != id).map(|s| (s.id, s.display(lang))).collect(),
            ),
            Target::Type(id) => (
                sum.type_by_id(id).map(|t| t.ticket_type.display(lang)).unwrap_or_default(),
                types::usage(cx.conn, id),
                sum.types.iter().filter(|t| t.ticket_type.id != id && !t.ticket_type.system).map(|t| (t.ticket_type.id, t.ticket_type.display(lang))).collect(),
            ),
            Target::Field(id) => {
                let f = sum.types.iter().flat_map(|t| t.fields.iter()).find(|f| f.id == id);
                let cands = f
                    .map(|f| sum.type_by_id(f.type_id).into_iter().flat_map(|t| t.fields.iter()).filter(|o| o.id != f.id && o.kind == f.kind).map(|o| (o.id, o.display(lang))).collect())
                    .unwrap_or_default();
                (f.map(|f| f.display(lang)).unwrap_or_default(), fields::usage(cx.conn, id), cands)
            }
            Target::LinkType(id) => {
                let me = self.links.iter().find(|l| l.id == id).cloned();
                let cands = me
                    .as_ref()
                    .map(|m| self.links.iter().filter(|o| o.id != m.id && o.target_kind == m.target_kind && o.symmetric == m.symmetric).map(|o| (o.id, o.names(lang).0)).collect())
                    .unwrap_or_default();
                (me.map(|m| m.names(lang).0).unwrap_or_default(), link_types::usage(cx.conn, id), cands)
            }
            Target::WorkKind(id) => (
                sum.work_kinds.iter().find(|w| w.id == id).map(|w| w.display(lang)).unwrap_or_default(),
                work_kinds::usage(cx.conn, id),
                sum.work_kinds.iter().filter(|w| w.id != id).map(|w| (w.id, w.display(lang))).collect(),
            ),
        };
        let usage = match usage {
            Ok(u) => u,
            Err(e) => {
                self.fail(cx, &e);
                return;
            }
        };
        let to = cands.first().map(|c| c.0);
        let mut d = Dlg { target, name, usage, replace: usage > 0 && to.is_some(), to, cands, map: Vec::new(), err: None };
        d.refresh_map(cx);
        self.del = Some(d);
    }

    pub(super) fn delete_dialog(&mut self, ctx: &egui::Context, cx: &Cx) {
        let Some(mut d) = self.del.take() else { return };
        let lang = cx.lang;
        let sum = self.sum.clone();
        let (noun, used_by, on_discard, on_plain) = words(d.target, lang);
        let mut go = false;
        let mut close = false;
        let mut remap = false;
        let m = egui::Modal::new(Self::modal_id("delete")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 520.0));
            ui.horizontal(|ui| {
                ui.colored_label(widgets::DANGER, RichText::new("!").heading().strong());
                let title = match lang {
                    Lang::De => format!("{noun} {} löschen?", lang.q(&d.name)),
                    Lang::En => format!("Delete {noun} {}?", lang.q(&d.name)),
                };
                ui.label(RichText::new(title).heading());
            });
            ui.add_space(S2);
            if d.usage == 0 {
                ui.label(on_plain);
                ui.add_space(S1);
                ui.label(lang.t("Das lässt sich nicht rückgängig machen.", "This cannot be undone."));
            } else {
                ui.label(RichText::new(format!("{} {used_by}", d.usage)).strong());
                ui.add_space(S1);
                ui.label(lang.t("Ersatz wählen oder verwerfen:", "Choose a replacement or discard:"));
                ui.add_space(S1);
                ui.add_enabled_ui(!d.cands.is_empty(), |ui| {
                    if widgets::radio(ui, &mut d.replace, true, lang.t("Auf einen Ersatz umhängen:", "Move to a replacement:")).changed() {
                        remap = true;
                    }
                });
                if d.replace {
                    ui.indent("del_replace", |ui| {
                        let sel = d.cands.iter().find(|c| Some(c.0) == d.to).map(|c| c.1.clone()).unwrap_or_default();
                        egui::ComboBox::from_id_salt("del_to").selected_text(sel).width(260.0).show_ui(ui, |ui| {
                            for (id, name) in &d.cands {
                                if ui.selectable_value(&mut d.to, Some(*id), name).changed() {
                                    remap = true;
                                }
                            }
                        });
                        if let (Target::Type(_), Some(sum)) = (d.target, &sum) {
                            ui.add_space(S1);
                            ui.label(RichText::new(lang.t("Felder zuordnen", "Map the fields")).small().strong());
                            let to_fields: Vec<(i64, String, FieldKind)> = d.to.and_then(|t| sum.type_by_id(t)).map(|t| t.fields.iter().map(|f| (f.id, f.display(lang), f.kind)).collect()).unwrap_or_default();
                            let from_fields: Vec<(i64, String, FieldKind)> = match d.target {
                                Target::Type(id) => sum.type_by_id(id).map(|t| t.fields.iter().map(|f| (f.id, f.display(lang), f.kind)).collect()).unwrap_or_default(),
                                _ => Vec::new(),
                            };
                            if from_fields.is_empty() {
                                ui.label(RichText::new(lang.t("Der Typ hat keine Felder.", "The type has no fields.")).small());
                            }
                            for (fid, fname, kind) in &from_fields {
                                let used: Vec<i64> = d.map.iter().filter(|e| e.from_field != *fid).filter_map(|e| e.to_field).collect();
                                let entry = d.map.iter_mut().find(|e| e.from_field == *fid);
                                let Some(entry) = entry else { continue };
                                ui.horizontal(|ui| {
                                    ui.label(format!("{fname} →"));
                                    let sel = entry.to_field.and_then(|t| to_fields.iter().find(|x| x.0 == t)).map(|x| x.1.clone()).unwrap_or_else(|| lang.t("Wert verwerfen", "Discard value").to_string());
                                    egui::ComboBox::from_id_salt(("del_map", *fid)).selected_text(sel).width(200.0).show_ui(ui, |ui| {
                                        ui.selectable_value(&mut entry.to_field, None, lang.t("Wert verwerfen", "Discard value"));
                                        for (tid, tname, tkind) in &to_fields {
                                            if tkind == kind && !used.contains(tid) {
                                                ui.selectable_value(&mut entry.to_field, Some(*tid), tname);
                                            }
                                        }
                                    });
                                });
                            }
                        }
                    });
                }
                if d.cands.is_empty() {
                    d.replace = false;
                    ui.label(RichText::new(lang.t("Es gibt keinen passenden Ersatz.", "There is no suitable replacement.")).small().color(egui::Color32::from_rgb(0xe5, 0xa0, 0x4b)));
                }
                widgets::radio(ui, &mut d.replace, false, lang.t("Verwerfen:", "Discard:"));
                ui.indent("del_discard", |ui| {
                    ui.label(RichText::new(on_discard).small());
                });
            }
            ui.add_space(S2);
            if let Some(e) = &d.err {
                widgets::error(ui, e);
                ui.add_space(S1);
            }
            ui.add_space(S1);
            ui.horizontal(|ui| {
                if widgets::danger(ui, lang.t("Löschen", "Delete"), 0).clicked() {
                    go = true;
                }
                if widgets::secondary(ui, lang.t("Abbrechen", "Cancel")).clicked() {
                    close = true;
                }
            });
        });
        if m.should_close() {
            close = true;
        }
        if remap {
            d.refresh_map(cx);
        }
        if go {
            match d.execute(cx) {
                Ok(()) => {
                    self.dirty = true;
                    self.forget(d.target);
                    self.note(match lang {
                        Lang::De => format!("{noun} {} gelöscht.", lang.q(&d.name)),
                        Lang::En => format!("Deleted {noun} {}.", lang.q(&d.name)),
                    });
                    close = true;
                }
                Err(e) => d.err = Some(e.message(lang)),
            }
        }
        if !close {
            self.del = Some(d);
        }
    }

    /// Auswahlen in den Editoren zurücksetzen, die auf etwas Gelöschtes zeigen.
    fn forget(&mut self, t: Target) {
        if let Target::Type(id) = t {
            if self.types.sel == Some(id) {
                self.types.sel = None;
            }
            if self.fields.ty == Some(id) {
                self.fields.ty = None;
            }
            if self.workflow.ty == Some(id) {
                self.workflow.ty = None;
            }
        }
    }
}

impl Dlg {
    /// Zuordnung der Felder neu vorschlagen (nur Typen).
    fn refresh_map(&mut self, cx: &Cx) {
        if let (Target::Type(from), Some(to)) = (self.target, self.to) {
            self.map = types::suggest_field_map(cx.conn, from, to).unwrap_or_default();
        } else {
            self.map.clear();
        }
    }

    fn execute(&self, cx: &Cx) -> Result<(), PlanError> {
        let mode = if self.usage == 0 {
            None
        } else if self.replace {
            Some(DeleteMode::Migrate { to: self.to.ok_or(PlanError::ReplacementInvalid)? })
        } else {
            Some(DeleteMode::Discard)
        };
        match self.target {
            Target::Status(id) => statuses::delete_status(cx.conn, id, mode),
            Target::Field(id) => fields::delete_field(cx.conn, id, mode),
            Target::LinkType(id) => link_types::delete_link_type(cx.conn, id, mode),
            Target::WorkKind(id) => work_kinds::delete_work_kind(cx.conn, id, mode),
            Target::Type(id) => {
                let tm = match mode {
                    None => None,
                    Some(DeleteMode::Discard) => Some(TypeDeleteMode::Discard),
                    Some(DeleteMode::Migrate { to }) => Some(TypeDeleteMode::Migrate(TypeMigration { to_type: to, fields: self.map.clone() })),
                };
                types::delete_type(cx.conn, id, tm)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::fields::{create_field, FieldKind, NewField};
    use crate::planning::integrity::test_hook;
    use crate::planning::projects::{create_project, NewProject};
    use crate::planning::statuses::{create_status, list_statuses, NewStatus, StatusCategory};
    use crate::planning::types::{create_type, list_types, NewType};

    /// Ein Dialog ohne Oberfläche, nur mit den Daten, die `execute` braucht.
    fn dlg(target: Target, usage: i64, replace: bool, to: Option<i64>, map: Vec<FieldMapEntry>) -> Dlg {
        Dlg { target, name: String::new(), usage, replace, to, cands: Vec::new(), map, err: None }
    }

    fn cx<'a>(conn: &'a rusqlite::Connection, db: &'a crate::db::Db) -> Cx<'a> {
        Cx { conn, db, lang: Lang::En }
    }

    fn open_db() -> crate::db::Db {
        let d = std::env::temp_dir().join(format!("paneldeck-test-delui-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        crate::db::open(&d, &d.join("backups")).unwrap()
    }

    #[test]
    fn unused_entries_delete_without_a_mode() {
        let db = open_db();
        let p = create_project(&db.conn, &NewProject::new("AB", "P", "#112233")).unwrap();
        let s = create_status(&db.conn, &NewStatus { project_id: p, key: None, name: Some("X".into()), color: "#445566".into(), category: StatusCategory::Todo }).unwrap();
        dlg(Target::Status(s), 0, false, None, Vec::new()).execute(&cx(&db.conn, &db)).unwrap();
        assert!(list_statuses(&db.conn, p).unwrap().is_empty());
    }

    #[test]
    fn used_entries_need_a_valid_choice() {
        let db = open_db();
        let p = create_project(&db.conn, &NewProject::new("AB", "P", "#112233")).unwrap();
        let mk = |n: &str| create_status(&db.conn, &NewStatus { project_id: p, key: None, name: Some(n.into()), color: "#445566".into(), category: StatusCategory::Todo }).unwrap();
        let (a, b) = (mk("A"), mk("B"));
        // „in Benutzung“ simulieren (die Ticket-Tabellen gibt es noch nicht)
        test_hook::add("status", "SELECT 2 + 0 * ?1");
        // Ersatz fehlt → Fehler, nichts gelöscht
        let r = dlg(Target::Status(a), 2, true, None, Vec::new()).execute(&cx(&db.conn, &db));
        assert_eq!(r, Err(PlanError::ReplacementInvalid));
        assert_eq!(list_statuses(&db.conn, p).unwrap().len(), 2);
        // sich selbst als Ersatz ist unzulässig
        let r = dlg(Target::Status(a), 2, true, Some(a), Vec::new()).execute(&cx(&db.conn, &db));
        assert_eq!(r, Err(PlanError::ReplacementInvalid));
        // gültiger Ersatz
        dlg(Target::Status(a), 2, true, Some(b), Vec::new()).execute(&cx(&db.conn, &db)).unwrap();
        assert_eq!(list_statuses(&db.conn, p).unwrap().len(), 1);
        // Verwerfen
        dlg(Target::Status(b), 2, false, None, Vec::new()).execute(&cx(&db.conn, &db)).unwrap();
        assert!(list_statuses(&db.conn, p).unwrap().is_empty());
    }

    #[test]
    fn type_deletion_carries_the_field_mapping() {
        let db = open_db();
        let p = create_project(&db.conn, &NewProject::new("AB", "P", "#112233")).unwrap();
        let mk_type = |n: &str| create_type(&db.conn, &NewType { project_id: p, key: None, name: Some(n.into()), icon: None, color: "#445566".into(), workflow: false }).unwrap();
        let (from, to) = (mk_type("Alt"), mk_type("Neu"));
        let f_from = create_field(&db.conn, &NewField::new(from, "Aufwand", FieldKind::Number)).unwrap();
        let f_to = create_field(&db.conn, &NewField::new(to, "Aufwand", FieldKind::Number)).unwrap();
        let mut d = dlg(Target::Type(from), 1, true, Some(to), Vec::new());
        d.refresh_map(&cx(&db.conn, &db));
        assert_eq!(d.map, vec![FieldMapEntry { from_field: f_from, to_field: Some(f_to) }], "gleichnamiges Feld mit gleichem Feldtyp wird vorgeschlagen");
        test_hook::add("type", "SELECT 1 + 0 * ?1");
        d.execute(&cx(&db.conn, &db)).unwrap();
        let left: Vec<i64> = list_types(&db.conn, p).unwrap().iter().map(|t| t.id).collect();
        assert_eq!(left, vec![to]);
        // ohne Ersatz-Typ gibt es keine Zuordnung
        let mut d = dlg(Target::Type(to), 1, false, None, Vec::new());
        d.refresh_map(&cx(&db.conn, &db));
        assert!(d.map.is_empty());
    }

    #[test]
    fn every_target_has_words_in_both_languages() {
        for t in [Target::Status(1), Target::Type(1), Target::Field(1), Target::LinkType(1), Target::WorkKind(1)] {
            let (de, en) = (words(t, Lang::De), words(t, Lang::En));
            assert_ne!(de.0, en.0, "{t:?}");
            assert!(!de.1.is_empty() && !de.2.is_empty() && !de.3.is_empty() && !en.1.is_empty() && !en.2.is_empty() && !en.3.is_empty());
        }
    }
}
