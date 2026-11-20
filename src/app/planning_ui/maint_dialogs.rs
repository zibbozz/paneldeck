//! Dialoge der Wartungsseite (#308): Vorschau einer Aktion (betroffene Tickets anklickbar, Kommentar änderbar, 3-Sekunden-Knopf
//! bei mehr als zehn Tickets oder beim Entfernen), Vergleich möglicher Duplikate nebeneinander, Reparatur mit Vorschau,
//! Verdichten (Hinweis: ganze Datei), Rettung in eine neue Datei und der Editor der eigenen Regeln.

use super::maint_view::{build_action, fix_label};
use super::ticket_state::countdown_secs;
use super::widgets::{self, hint};
use super::{Cx, PlanUi};
use crate::planning::fields::FieldKind;
use crate::planning::maintenance::actions::{self, Action, Preview};
use crate::planning::maintenance::rules::{NewRule, Rule, RuleAction};
use crate::planning::maintenance::{self as maint, Check, Finding, Fix, Severity, TicketRef};
use crate::state::Lang;
use crate::style::{S1, S2};
use eframe::egui::{self, RichText};
use serde_json::Value;
use std::time::Instant;

pub(super) struct FixDlg {
    fix: Fix,
    findings: Vec<Finding>,
    comment: String,
    due: String,
    since: Instant,
}

pub(super) struct CompareDlg {
    finding: Finding,
    a: TicketRef,
    b: TicketRef,
    comment: String,
}

pub(super) struct RuleForm {
    id: Option<i64>,
    name: String,
    jql: String,
    severity: Severity,
    /// 0 nur melden, 1 archivieren, 2 Status, 3 Feld
    kind: u8,
    status: Option<i64>,
    field: Option<i64>,
    value: String,
    enabled: bool,
    err: Option<String>,
    delete_since: Option<Instant>,
}

impl RuleForm {
    pub fn new(r: Option<&Rule>) -> RuleForm {
        let mut f = RuleForm { id: None, name: String::new(), jql: String::new(), severity: Severity::Warning, kind: 0, status: None, field: None, value: String::new(), enabled: true, err: None, delete_since: None };
        if let Some(r) = r {
            f.id = Some(r.id);
            f.name = r.name.clone();
            f.jql = r.jql.clone();
            f.severity = r.severity;
            f.enabled = r.enabled;
            match &r.action {
                RuleAction::None => {}
                RuleAction::Archive => f.kind = 1,
                RuleAction::Status { status } => {
                    f.kind = 2;
                    f.status = Some(*status);
                }
                RuleAction::Field { field, value } => {
                    f.kind = 3;
                    f.field = Some(*field);
                    f.value = match value {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                }
            }
        }
        f
    }
}

/// Text in einen Feldwert der Regel-Aktion übersetzen (Text, Zahl, Ja/Nein, Datum).
pub(super) fn rule_value(kind: FieldKind, text: &str) -> Option<Value> {
    let t = text.trim();
    match kind {
        FieldKind::Number => t.replace(',', ".").parse::<f64>().ok().map(crate::planning::values::canon_number),
        FieldKind::Checkbox => match t.to_lowercase().as_str() {
            "true" | "yes" | "ja" | "1" => Some(Value::Bool(true)),
            "false" | "no" | "nein" | "0" => Some(Value::Bool(false)),
            _ => None,
        },
        FieldKind::Text | FieldKind::Rich | FieldKind::Date | FieldKind::Datetime => Some(Value::String(t.to_string())),
        _ => None,
    }
}

#[derive(Default)]
pub(super) struct Dialogs {
    pub fix: Option<FixDlg>,
    pub compare: Option<CompareDlg>,
    pub rule: Option<RuleForm>,
    pub repair: bool,
    pub compact: bool,
    pub rescue: bool,
    pub since: Option<Instant>,
}

impl Dialogs {
    pub fn any_open(&self) -> bool {
        self.fix.is_some() || self.compare.is_some() || self.rule.is_some() || self.repair || self.compact || self.rescue
    }
}

impl PlanUi {
    /// Aktion für Befunde öffnen: Vergleich (Duplikat), Reparatur (Datenbank) oder die Vorschau.
    pub(super) fn maint_open_fix(&mut self, cx: &Cx, fix: Fix, findings: &[&Finding]) {
        let lang = cx.lang;
        match fix {
            Fix::LinkDuplicate => {
                let Some(f) = findings.first() else { return };
                let (Some(a), Some(b)) = (f.ticket.clone(), f.other.clone()) else { return };
                let reason = actions::reason_text(lang, Check::PossibleDuplicate, None);
                self.maint.dlg.compare = Some(CompareDlg { finding: (*f).clone(), a, b, comment: actions::default_comment(lang, chrono::Local::now().date_naive(), &reason) });
            }
            Fix::Repair => match maint::dbcare::plan_repair(cx.conn) {
                Ok(p) => {
                    self.maint.repair_plan = Some(p);
                    self.maint.dlg.repair = true;
                    self.maint.dlg.since = Some(Instant::now());
                }
                Err(e) => self.maint.msg = Some((e.message(lang), false)),
            },
            _ => {
                let same = findings.iter().all(|f| f.check == findings[0].check);
                let days = if findings.len() == 1 { findings[0].detail.get("days").and_then(|d| d.as_i64()) } else { None };
                let reason = actions::reason_text(lang, if same { findings[0].check } else { Check::CustomRule }, days);
                let due = (chrono::Local::now().date_naive() + chrono::Duration::days(14)).format("%Y-%m-%d").to_string();
                self.maint.dlg.fix = Some(FixDlg { fix, findings: findings.iter().map(|f| (*f).clone()).collect(), comment: actions::default_comment(lang, chrono::Local::now().date_naive(), &reason), due, since: Instant::now() });
            }
        }
    }

    pub(super) fn maint_dialogs(&mut self, ctx: &egui::Context, cx: &Cx) {
        if !self.maint.open {
            return;
        }
        self.maint_fix_dialog(ctx, cx);
        self.maint_compare_dialog(ctx, cx);
        self.maint_rule_dialog(ctx, cx);
        self.maint_db_dialogs(ctx, cx);
    }

    fn maint_fix_dialog(&mut self, ctx: &egui::Context, cx: &Cx) {
        let Some(mut d) = self.maint.dlg.fix.take() else { return };
        let lang = cx.lang;
        let refs: Vec<&Finding> = d.findings.iter().collect();
        let action = build_action(d.fix, &refs, d.comment.clone(), d.due.trim().to_string());
        let preview: Result<Preview, String> = match &action {
            Some(a) => actions::preview(cx.conn, self.maint.project, a, lang).map_err(|e| e.message(lang)),
            None => Err(lang.t("Diese Auswahl lässt sich nicht gemeinsam beheben.", "This selection cannot be fixed together.").into()),
        };
        let (mut go, mut close, mut open_ticket, mut settings) = (false, false, None, false);
        let m = crate::style::modal(Self::modal_id("maint_fix")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 620.0));
            widgets::dialog_title(ui, fix_label(d.fix, lang).trim_end_matches(" …"));
            match &preview {
                Err(e) => {
                    widgets::error(ui, e);
                    if matches!(d.fix, Fix::CloseWontDo) {
                        if ui.link(lang.t("Zielstatus in den Projekt-Einstellungen wählen …", "Choose the target status in the project settings …")).clicked() {
                            settings = true;
                        }
                    }
                }
                Ok(p) => {
                    ui.label(RichText::new(&p.headline).strong());
                    ui.add_space(S1);
                    egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                        for t in &p.tickets {
                            ui.horizontal(|ui| {
                                if ui.link(RichText::new(&t.key).monospace()).clicked() {
                                    open_ticket = Some(t.id);
                                }
                                ui.label(RichText::new(&t.title).small());
                            });
                        }
                        for l in &p.lines {
                            ui.label(RichText::new(l).small());
                        }
                    });
                }
            }
            if d.fix == Fix::SetDue {
                ui.add_space(S1);
                ui.horizontal(|ui| {
                    ui.label(lang.t("Fällig am (JJJJ-MM-TT):", "Due on (YYYY-MM-DD):"));
                    ui.text_edit_singleline(&mut d.due);
                });
            }
            if d.fix == Fix::CloseWontDo {
                ui.add_space(S1);
                ui.label(lang.t("Kommentar (wird an jedes Ticket geschrieben):", "Comment (added to every ticket):"));
                ui.add(egui::TextEdit::multiline(&mut d.comment).desired_rows(2).desired_width(f32::INFINITY));
            }
            ui.add_space(S1);
            hint(ui, lang.t("Vorher legt Paneldeck eine Sicherung „maintenance“ an. Die Wartung löscht nie Tickets; zurück geht es über die Sicherung.", "Paneldeck makes a “maintenance” backup first. Maintenance never deletes tickets; to undo, restore the backup."));
            ui.add_space(S2);
            ui.horizontal(|ui| {
                let ok = preview.as_ref().is_ok_and(|p| p.count > 0);
                let slow = preview.as_ref().is_ok_and(|p| p.confirm_slow);
                let wait = if slow { countdown_secs(d.since, Instant::now()) } else { 0 };
                if wait > 0 {
                    ctx.request_repaint_after(std::time::Duration::from_millis(200));
                }
                let r = if slow { ok && widgets::danger(ui, lang.t("Ausführen", "Run"), wait).clicked() } else { widgets::primary_enabled(ui, ok, lang.t("Ausführen", "Run")).clicked() };
                if r {
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
        if settings {
            if let Some(p) = self.project().cloned() {
                self.open_settings(&p);
            }
            close = true;
        }
        if let Some(t) = open_ticket {
            self.open_ticket_anywhere(cx, t, self.maint.project);
            close = true;
        }
        if go {
            if let Some(a) = action {
                self.maint_spawn_action(cx, a, fix_label(d.fix, lang).trim_end_matches(" …").to_string());
            }
            close = true;
        }
        if !close {
            self.maint.dlg.fix = Some(d);
        }
    }

    fn maint_compare_dialog(&mut self, ctx: &egui::Context, cx: &Cx) {
        let Some(mut d) = self.maint.dlg.compare.take() else { return };
        let lang = cx.lang;
        let (mut close, mut act): (bool, Option<bool>) = (false, None);
        let mut ignore = false;
        let has_wont_do = maint::load_settings(cx.conn, self.maint.project).map(|s| s.wont_do.is_some()).unwrap_or(false);
        // Das jüngere Ticket (höhere Kennung) ist das Duplikat des älteren.
        let (dup, orig) = if d.a.id > d.b.id { (d.a.clone(), d.b.clone()) } else { (d.b.clone(), d.a.clone()) };
        let m = crate::style::modal(Self::modal_id("maint_compare")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 760.0));
            widgets::dialog_title(ui, lang.t("Mögliches Duplikat vergleichen", "Compare a possible duplicate"));
            ui.label(RichText::new(&d.finding.message).small());
            ui.add_space(S1);
            ui.columns(2, |cols| {
                for (ui, t) in cols.iter_mut().zip([&d.a, &d.b]) {
                    widgets::card(ui, |ui| ticket_side(ui, cx, t.id, lang));
                }
            });
            ui.add_space(S1);
            ui.label(RichText::new(match lang {
                Lang::De => format!("„Als Duplikat verknüpfen“: {} dupliziert {} (Verknüpfungstyp „duplicates“, wird bei Bedarf angelegt).", dup.key, orig.key),
                Lang::En => format!("“Link as duplicate”: {} duplicates {} (link type “duplicates”, created when needed).", dup.key, orig.key),
            }).small());
            if has_wont_do {
                ui.label(lang.t("Kommentar beim Schließen:", "Comment when closing:"));
                ui.add(egui::TextEdit::multiline(&mut d.comment).desired_rows(2).desired_width(f32::INFINITY));
            } else {
                hint(ui, lang.t("„Verknüpfen und schließen“ braucht einen Zielstatus „Won't Do“ (Projekt-Einstellungen).", "“Link and close” needs a “Won't Do” target status (project settings)."));
            }
            ui.add_space(S2);
            ui.horizontal(|ui| {
                if widgets::primary(ui, lang.t("Als Duplikat verknüpfen", "Link as duplicate")).clicked() {
                    act = Some(false);
                }
                if ui.add_enabled(has_wont_do, egui::Button::new(lang.t("Verknüpfen und schließen", "Link and close"))).clicked() {
                    act = Some(true);
                }
                if ui.button(lang.t("Kein Duplikat (ignorieren)", "Not a duplicate (ignore)")).clicked() {
                    ignore = true;
                }
                if widgets::secondary(ui, lang.t("Abbrechen", "Cancel")).clicked() {
                    close = true;
                }
            });
        });
        if m.should_close() {
            close = true;
        }
        if ignore {
            let _ = maint::ignore(cx.conn, self.maint.project, &[d.finding.id.clone()]);
            self.maint.ignored = None;
            self.maint_run_check(cx);
            close = true;
        }
        if let Some(c) = act {
            let a = Action::LinkDuplicate { ticket: dup.id, original: orig.id, close: c, comment: d.comment.clone() };
            self.maint_spawn_action(cx, a, lang.t("Als Duplikat verknüpft", "Linked as duplicate").into());
            close = true;
        }
        if !close {
            self.maint.dlg.compare = Some(d);
        }
    }

    fn maint_db_dialogs(&mut self, ctx: &egui::Context, cx: &Cx) {
        let lang = cx.lang;
        if self.maint.dlg.repair {
            let plan = self.maint.repair_plan.clone().unwrap_or_default();
            let (mut go, mut close) = (false, false);
            let m = crate::style::modal(Self::modal_id("maint_repair")).show(ctx, |ui| {
                ui.set_width(widgets::modal_width(ctx, 600.0));
                widgets::dialog_title(ui, lang.t("Datenbank reparieren", "Repair the database"));
                if plan.is_empty() {
                    ui.label(lang.t("Nichts zu reparieren – die eigenen Prüfungen sind in Ordnung.", "Nothing to repair – the own checks are fine."));
                } else {
                    hint(ui, lang.t("Repariert wird nur, was sich eindeutig wiederherstellen lässt. Vorher entsteht eine Sicherung „maintenance“.", "Only what can be rebuilt unambiguously is repaired. A “maintenance” backup is made first."));
                    for s in &plan {
                        let t = s.text(lang);
                        if s.applies() {
                            ui.label(format!("• {t}"));
                        } else {
                            ui.label(RichText::new(format!("• {t}")).weak());
                        }
                    }
                }
                ui.add_space(S2);
                ui.horizontal(|ui| {
                    if widgets::primary_enabled(ui, plan.iter().any(|s| s.applies()), lang.t("Reparieren", "Repair")).clicked() {
                        go = true;
                    }
                    if widgets::secondary(ui, lang.t("Abbrechen", "Cancel")).clicked() {
                        close = true;
                    }
                });
            });
            if m.should_close() || close {
                self.maint.dlg.repair = false;
            }
            if go {
                self.maint.dlg.repair = false;
                self.maint_spawn_repair(cx, plan);
            }
        }
        if self.maint.dlg.compact {
            let (mut go, mut close) = (false, false);
            let size = maint::dbcare::file_size(&cx.db.path);
            let m = crate::style::modal(Self::modal_id("maint_compact")).show(ctx, |ui| {
                ui.set_width(widgets::modal_width(ctx, 520.0));
                widgets::dialog_title(ui, lang.t("Datenbank verdichten", "Compact the database"));
                ui.label(lang.t(
                    "VACUUM, ANALYZE und REINDEX über die ganze Datei (alle Projekte). Schreibzugriffe sind kurz gesperrt; vorher entsteht eine Sicherung „maintenance“.",
                    "VACUUM, ANALYZE and REINDEX over the whole file (all projects). Writes are locked briefly; a “maintenance” backup is made first.",
                ));
                ui.label(RichText::new(format!("{} {}", lang.t("Größe jetzt:", "Size now:"), maint::dbcare::human_bytes(size as i64))).small());
                ui.add_space(S2);
                ui.horizontal(|ui| {
                    if widgets::primary(ui, lang.t("Verdichten", "Compact")).clicked() {
                        go = true;
                    }
                    if widgets::secondary(ui, lang.t("Abbrechen", "Cancel")).clicked() {
                        close = true;
                    }
                });
            });
            if m.should_close() || close {
                self.maint.dlg.compact = false;
            }
            if go {
                self.maint.dlg.compact = false;
                self.maint_spawn_compact(cx);
            }
        }
        if self.maint.dlg.rescue {
            let (mut go, mut close) = (false, false);
            let m = crate::style::modal(Self::modal_id("maint_rescue")).show(ctx, |ui| {
                ui.set_width(widgets::modal_width(ctx, 560.0));
                widgets::dialog_title(ui, lang.t("Daten in eine neue Datei retten", "Rescue the data into a new file"));
                ui.label(lang.t(
                    "Liest alles Lesbare Zeile für Zeile und schreibt es in eine neue Datei neben der Datenbank (paneldeck-rescued-…db) samt Bericht. Die Datenbank selbst bleibt unverändert. Danach kannst du die gerettete Datei wie eine Sicherung verwenden.",
                    "Reads everything readable row by row and writes it into a new file next to the database (paneldeck-rescued-…db) with a report. The database itself stays unchanged. Afterwards you can use the rescued file like a backup.",
                ));
                ui.add_space(S2);
                ui.horizontal(|ui| {
                    if widgets::primary(ui, lang.t("Retten", "Rescue")).clicked() {
                        go = true;
                    }
                    if widgets::secondary(ui, lang.t("Abbrechen", "Cancel")).clicked() {
                        close = true;
                    }
                });
            });
            if m.should_close() || close {
                self.maint.dlg.rescue = false;
            }
            if go {
                self.maint.dlg.rescue = false;
                self.maint_spawn_rescue(cx);
            }
        }
    }

    fn maint_rule_dialog(&mut self, ctx: &egui::Context, cx: &Cx) {
        let Some(mut f) = self.maint.dlg.rule.take() else { return };
        let lang = cx.lang;
        let project = self.maint.project;
        let statuses = crate::planning::statuses::list_statuses(cx.conn, project).unwrap_or_default();
        let mut fields: Vec<(i64, String, FieldKind)> = Vec::new();
        for ty in crate::planning::types::list_types(cx.conn, project).unwrap_or_default() {
            for fl in crate::planning::fields::list_fields(cx.conn, ty.id).unwrap_or_default() {
                if matches!(fl.kind, FieldKind::Text | FieldKind::Rich | FieldKind::Number | FieldKind::Checkbox | FieldKind::Date | FieldKind::Datetime) {
                    fields.push((fl.id, format!("{} · {}", ty.display(lang), fl.display(lang)), fl.kind));
                }
            }
        }
        let query_err = crate::planning::search::prepare(cx.conn, &crate::planning::search::Ctx::today_local(lang), crate::planning::search::Scope::Project(project), &f.jql).err().map(|e| e.message(lang));
        let (mut save, mut close, mut delete) = (false, false, false);
        let m = crate::style::modal(Self::modal_id("maint_rule")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 600.0));
            widgets::dialog_title(ui, if f.id.is_some() { lang.t("Regel bearbeiten", "Edit rule") } else { lang.t("Neue Prüfregel", "New check rule") });
            ui.label(RichText::new(lang.t("Name", "Name")).strong());
            ui.add(egui::TextEdit::singleline(&mut f.name).desired_width(f32::INFINITY));
            ui.add_space(S1);
            ui.label(RichText::new(lang.t("Abfrage (trifft die Tickets, die gemeldet werden)", "Query (matches the tickets to report)")).strong());
            ui.add(egui::TextEdit::singleline(&mut f.jql).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace).hint_text("type = Bug AND priority IS EMPTY"));
            if let Some(e) = query_err.as_ref().filter(|_| !f.jql.trim().is_empty()) {
                widgets::error(ui, e);
            }
            ui.add_space(S1);
            ui.horizontal(|ui| {
                ui.label(RichText::new(lang.t("Schwere", "Severity")).strong());
                for s in Severity::ALL {
                    widgets::radio(ui, &mut f.severity, s, s.label(lang));
                }
            });
            ui.add_space(S1);
            ui.label(RichText::new(lang.t("Vorgeschlagene Aktion", "Suggested action")).strong());
            ui.horizontal_wrapped(|ui| {
                widgets::radio(ui, &mut f.kind, 0, lang.t("Nur melden", "Report only"));
                widgets::radio(ui, &mut f.kind, 1, lang.t("Archivieren", "Archive"));
                widgets::radio(ui, &mut f.kind, 2, lang.t("Status setzen", "Set status"));
                widgets::radio(ui, &mut f.kind, 3, lang.t("Feld setzen", "Set field"));
            });
            if f.kind == 2 {
                let sel = f.status.and_then(|s| statuses.iter().find(|x| x.id == s)).map(|s| s.display(lang)).unwrap_or_else(|| lang.t("– wählen –", "– choose –").into());
                egui::ComboBox::from_id_salt("maint_rule_status").selected_text(sel).show_ui(ui, |ui| {
                    for s in &statuses {
                        ui.selectable_value(&mut f.status, Some(s.id), s.display(lang));
                    }
                });
            }
            if f.kind == 3 {
                let sel = f.field.and_then(|id| fields.iter().find(|x| x.0 == id)).map(|x| x.1.clone()).unwrap_or_else(|| lang.t("– wählen –", "– choose –").into());
                egui::ComboBox::from_id_salt("maint_rule_field").selected_text(sel).width(320.0).show_ui(ui, |ui| {
                    for (id, name, _) in &fields {
                        ui.selectable_value(&mut f.field, Some(*id), name);
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(lang.t("Wert:", "Value:"));
                    ui.text_edit_singleline(&mut f.value);
                });
            }
            ui.add_space(S1);
            widgets::check(ui, &mut f.enabled, lang.t("Regel ist an", "Rule is on"));
            if let Some(e) = &f.err {
                widgets::error(ui, e);
            }
            ui.add_space(S2);
            ui.horizontal(|ui| {
                if widgets::primary(ui, lang.t("Speichern", "Save")).clicked() {
                    save = true;
                }
                if widgets::secondary(ui, lang.t("Abbrechen", "Cancel")).clicked() {
                    close = true;
                }
                if f.id.is_some() {
                    let since = *f.delete_since.get_or_insert_with(Instant::now);
                    let wait = countdown_secs(since, Instant::now());
                    if wait > 0 {
                        ctx.request_repaint_after(std::time::Duration::from_millis(200));
                    }
                    if widgets::danger(ui, lang.t("Regel löschen", "Delete rule"), wait).clicked() {
                        delete = true;
                    }
                }
            });
        });
        if m.should_close() {
            close = true;
        }
        if delete {
            if let Some(id) = f.id {
                let _ = maint::rules::delete_rule(cx.conn, id);
            }
            self.maint.rules = None;
            self.maint_run_check(cx);
            close = true;
        }
        if save {
            let action = match f.kind {
                1 => Some(RuleAction::Archive),
                2 => f.status.map(|status| RuleAction::Status { status }),
                3 => f.field.and_then(|id| fields.iter().find(|x| x.0 == id)).and_then(|(id, _, kind)| rule_value(*kind, &f.value).map(|value| RuleAction::Field { field: *id, value })),
                _ => Some(RuleAction::None),
            };
            match action {
                None => f.err = Some(lang.t("Die Aktion ist unvollständig (Status/Feld wählen, gültigen Wert eingeben).", "The action is incomplete (choose a status/field, enter a valid value).").into()),
                Some(action) => {
                    let n = NewRule { name: f.name.clone(), jql: f.jql.clone(), severity: f.severity, action, enabled: f.enabled };
                    let r = match f.id {
                        Some(id) => maint::rules::update_rule(cx.conn, id, &n, lang),
                        None => maint::rules::create_rule(cx.conn, project, &n, lang).map(|_| ()),
                    };
                    match r {
                        Ok(()) => {
                            self.maint.rules = None;
                            self.maint_run_check(cx);
                            close = true;
                        }
                        Err(e) => f.err = Some(e.message(lang)),
                    }
                }
            }
        }
        if !close {
            self.maint.dlg.rule = Some(f);
        }
    }
}

/// Eine Seite des Vergleichs: Schlüssel, Titel, Status, Erstellt, Beschreibung.
fn ticket_side(ui: &mut egui::Ui, cx: &Cx, id: i64, lang: Lang) {
    let Ok(t) = crate::planning::tickets::get_ticket(cx.conn, id) else { return };
    ui.label(RichText::new(t.key()).monospace().strong());
    ui.label(RichText::new(&t.title).strong());
    let status = t.status_id.and_then(|s| crate::planning::statuses::get_status(cx.conn, s).ok()).map(|s| s.display(lang)).unwrap_or_else(|| "–".into());
    ui.label(RichText::new(format!("{}: {status} · {}: {}", lang.t("Status", "Status"), lang.t("Erstellt", "Created"), t.created_at.replace('T', " "))).small());
    let desc = crate::planning::fields::list_fields(cx.conn, t.type_id)
        .ok()
        .and_then(|fs| fs.into_iter().find(|f| f.is_description()))
        .and_then(|f| crate::planning::values::get_value(cx.conn, t.id, &f).ok().flatten())
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_default();
    ui.add_space(4.0);
    egui::ScrollArea::vertical().id_salt(("maint_cmp", id)).max_height(200.0).show(ui, |ui| {
        if desc.trim().is_empty() {
            ui.label(RichText::new(lang.t("– keine Beschreibung –", "– no description –")).weak());
        } else {
            ui.label(RichText::new(desc).small());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_values_by_kind() {
        assert_eq!(rule_value(FieldKind::Number, "1,5"), Some(serde_json::json!(1.5)));
        assert_eq!(rule_value(FieldKind::Checkbox, "ja"), Some(Value::Bool(true)));
        assert_eq!(rule_value(FieldKind::Checkbox, "vielleicht"), None);
        assert_eq!(rule_value(FieldKind::Date, " 2026-12-01 "), Some(Value::String("2026-12-01".into())));
        assert_eq!(rule_value(FieldKind::Dropdown, "x"), None);
        let _ = Action::Archive { tickets: vec![] };
        let _ = Check::ALL;
    }
}
