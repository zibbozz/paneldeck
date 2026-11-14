//! Reiter „Schnellaktionen“ der Projektansicht: Aktionen je Tickettyp anlegen, bearbeiten, löschen, ordnen und duplizieren.
//!
//! * Die Aktionen hängen am Typ (Geltungsbereich = der gewählte Typ; „Status ändern“ gilt nur dort, wo der Übergang erlaubt ist).
//! * Der Dialog zeigt je Aktionsart die passenden Felder (`create_ticket`: Ziel-Typ, Verknüpfung, Standardwerte;
//!   `change_status`: Zielstatus, zu setzende Werte; `start_session`: Shell, Claude, Prompt, Arbeitsordner) und prüft **live**
//!   mit `quick::actions::validate_params`; die Fehlertexte kommen aus der Datenschicht (zweisprachig).
//! * Eingebaute Aktionen (mit Schlüssel, z. B. aus Vorlagen) tragen die Marke „Standard“: ihr Name wird übersetzt, bis er
//!   geändert wird; sonst verhalten sie sich wie eigene.
//!
//! Die Dialog-Logik ([`Dlg`]) ist ohne Oberfläche testbar.

use super::helpers::{color_of, move_id};
use super::values::{blank_full, full_editor};
use super::widgets::{self, hint};
use super::{Cx, Msg, PlanUi};
use crate::planning::fields::{Field, FieldKind};
use crate::planning::link_types::{LinkType, TargetKind};
use crate::planning::quick::actions::{check_quick_action, create_quick_action, delete_quick_action, duplicate_quick_action, list_quick_actions, reorder_quick_actions, update_quick_action, validate_params, NewQuickAction, QuickAction, UpdateQuickAction};
use crate::planning::quick::params::{ActionKind, ActionParams, ChangeStatusParams, CreateTicketParams, DefaultValue, FieldRef, LinkDirection, RelationSpec, StartSessionParams};
use crate::planning::quick::resolve::{PLACEHOLDERS, SOURCE_REF};
use crate::planning::quick::{QRes, QuickError};
use crate::planning::sprints::releases::{list_releases, ReleaseKind};
use crate::planning::summary::{ProjectSummary, TypeSummary};
use crate::planning::workflow::SetValue;
use crate::state::Lang;
use crate::style::{self, Icon as SIcon, S1, S2};
use eframe::egui::{self, Align, Id, Layout, RichText, Ui, Vec2};
use serde_json::{json, Value};
use std::rc::Rc;
use std::time::Instant;

/// Ein Eintrag der Wertelisten: Titel oder Feld des Ziel-Typs, dazu der Wert.
pub(super) type Entry = (FieldRef, Value);

#[derive(Default)]
pub(super) struct State {
    sel_type: Option<i64>,
    list: Vec<Row>,
    loaded: Option<(i64, Rc<ProjectSummary>)>,
    stale: bool,
    pub(super) dlg: Option<Dlg>,
    confirm_delete: Option<i64>,
}

struct Row {
    action: QuickAction,
    /// Grund, warum die Aktion ungültig ist (Text der Datenschicht).
    problem: Option<String>,
}

/// Zustand des Bearbeiten-Dialogs.
pub(super) struct Dlg {
    /// `None` = neue Aktion.
    pub id: Option<i64>,
    pub type_id: i64,
    pub kind: ActionKind,
    pub name: String,
    /// Standardname (Aktion mit Schlüssel und ohne eigenen Namen): der Text darf unverändert bleiben.
    pub default_name: Option<String>,
    pub has_key: bool,
    // create_ticket
    pub target_type: i64,
    pub relation: Option<RelationSpec>,
    /// Standardwerte (create_ticket) bzw. zu setzende Werte (change_status).
    pub entries: Vec<Entry>,
    // change_status
    pub to_status: i64,
    // start_session
    pub shell: String,
    pub claude: bool,
    pub prompt: bool,
    pub cwd: bool,
    pub err: Option<String>,
    /// Stand der Eingaben, als `err` gesetzt wurde: ändert sich etwas, ist der Fehler überholt und verschwindet.
    pub err_fp: String,
    /// Der Name wurde schon bearbeitet: erst dann erscheint „Der Name darf nicht leer sein.“ in Rot (nicht gleich beim Öffnen).
    pub touched: bool,
}

impl Dlg {
    pub fn new_action(type_id: i64, kind: ActionKind, first_target: i64, first_status: i64) -> Dlg {
        Dlg {
            id: None,
            type_id,
            kind,
            name: String::new(),
            default_name: None,
            has_key: false,
            target_type: first_target,
            relation: None,
            entries: Vec::new(),
            to_status: first_status,
            shell: String::new(),
            claude: false,
            prompt: false,
            cwd: false,
            err: None,
            err_fp: String::new(),
            touched: false,
        }
    }

    pub fn from_action(a: &QuickAction, lang: Lang) -> Dlg {
        let mut d = Dlg::new_action(a.type_id, a.kind(), 0, 0);
        d.id = Some(a.id);
        d.name = a.display(lang);
        d.has_key = a.key.is_some();
        d.default_name = (a.key.is_some() && a.name.is_none()).then(|| d.name.clone());
        match &a.params {
            ActionParams::CreateTicket(c) => {
                d.target_type = c.ticket_type_id;
                d.relation = c.relation.clone();
                d.entries = c.defaults.iter().map(|x| (x.field_id, x.value.clone())).collect();
            }
            ActionParams::ChangeStatus(c) => {
                d.to_status = c.to_status_id;
                d.entries = c.set_values.iter().map(|s| (FieldRef::Field(s.field_id), s.value.clone())).collect();
            }
            ActionParams::StartSession(c) => {
                d.shell = c.shell.clone().unwrap_or_default();
                d.claude = c.claude;
                d.prompt = c.prompt_from_ticket;
                d.cwd = c.cwd_from_project;
            }
        }
        d
    }

    /// Die Parameter aus dem Dialog (ungeprüft; `validate_params` prüft).
    pub fn params(&self) -> ActionParams {
        match self.kind {
            ActionKind::CreateTicket => ActionParams::CreateTicket(CreateTicketParams {
                ticket_type_id: self.target_type,
                relation: self.relation.clone(),
                defaults: self.entries.iter().map(|(f, v)| DefaultValue { field_id: *f, value: v.clone() }).collect(),
            }),
            ActionKind::ChangeStatus => ActionParams::ChangeStatus(ChangeStatusParams {
                to_status_id: self.to_status,
                set_values: self.entries.iter().filter_map(|(f, v)| if let FieldRef::Field(id) = f { Some(SetValue { field_id: *id, value: v.clone() }) } else { None }).collect(),
            }),
            ActionKind::StartSession => ActionParams::StartSession(StartSessionParams {
                shell: Some(self.shell.trim().to_string()).filter(|s| !s.is_empty()),
                claude: self.claude,
                prompt_from_ticket: self.prompt,
                cwd_from_project: self.cwd,
            }),
        }
    }

    /// Name für `UpdateQuickAction`: unverändert bleibt der Standardname unberührt.
    pub fn name_for_update(&self) -> Option<String> {
        if self.default_name.as_deref() == Some(self.name.as_str()) { None } else { Some(self.name.clone()) }
    }

    /// Fehlt der Name noch (und die Aktion hat keinen Standardnamen)?
    pub fn name_missing(&self) -> bool {
        self.name.trim().is_empty() && !self.has_key
    }

    /// Stand der Eingaben für die Erkennung „seit dem Fehler geändert“.
    fn fingerprint(&self) -> String {
        format!("{}|{:?}", self.name, self.params())
    }

    /// Fehler des letzten Speicherversuchs merken (samt Stand der Eingaben).
    pub fn set_err(&mut self, e: String) {
        self.err = Some(e);
        self.err_fp = self.fingerprint();
    }

    /// Hat sich seit dem Fehler etwas geändert, ist er überholt: entfernen.
    pub fn drop_stale_err(&mut self) {
        if self.err.is_some() && self.err_fp != self.fingerprint() {
            self.err = None;
        }
    }

    /// Live-Prüfung: Fehlertext der Datenschicht (zweisprachig) oder `None`.
    pub fn problem(&self, conn: &rusqlite::Connection, lang: Lang) -> Option<String> {
        if self.name_missing() {
            return Some(crate::planning::PlanError::NameEmpty.message(lang));
        }
        validate_params(conn, self.type_id, &self.params()).err().map(|e| e.message(lang))
    }

    /// Eintrag für `field` anhängen (mit passendem Anfangswert); `false`, wenn nichts mehr frei ist.
    pub fn add_entry(&mut self, fields: &[Field], allow_title: bool) -> bool {
        let used: Vec<FieldRef> = self.entries.iter().map(|e| e.0).collect();
        let free = |f: FieldRef| !used.contains(&f);
        if allow_title && free(FieldRef::Title) {
            self.entries.push((FieldRef::Title, json!("")));
            return true;
        }
        match fields.iter().find(|f| f.kind != FieldKind::Terminal && free(FieldRef::Field(f.id))) {
            Some(f) => {
                self.entries.push((FieldRef::Field(f.id), start_value(f)));
                true
            }
            None => false,
        }
    }
}

/// Anfangswert eines Eintrags für das Feld.
fn start_value(f: &Field) -> Value {
    match f.kind {
        FieldKind::Ticket => json!(SOURCE_REF),
        FieldKind::Version | FieldKind::Milestone => Value::Null,
        k => blank_full(k, &f.options),
    }
}

fn kind_hint(k: ActionKind, lang: Lang) -> &'static str {
    match k {
        ActionKind::CreateTicket => lang.t("Legt ein neues Ticket an, optional verknüpft mit diesem.", "Creates a new ticket, optionally linked to this one."),
        ActionKind::ChangeStatus => lang.t("Setzt den Status (die Regeln des Workflows gelten) und schreibt optional Werte.", "Sets the status (workflow rules apply) and optionally writes values."),
        ActionKind::StartSession => lang.t("Startet eine Terminal- oder Claude-Sitzung für dieses Ticket.", "Starts a terminal or Claude session for this ticket."),
    }
}

/// Kurzbeschreibung einer Aktion für die Liste.
fn describe(a: &QuickAction, sum: &ProjectSummary, lang: Lang) -> String {
    match &a.params {
        ActionParams::CreateTicket(c) => {
            let t = sum.type_by_id(c.ticket_type_id).map(|t| t.ticket_type.display(lang)).unwrap_or_else(|| "?".into());
            let mut s = format!("{}: {t}", lang.t("Ticket anlegen", "Create ticket"));
            if c.relation.is_some() {
                s.push_str(lang.t(" · verknüpft", " · linked"));
            }
            s
        }
        ActionParams::ChangeStatus(c) => {
            let st = sum.status(c.to_status_id).map(|s| s.display(lang)).unwrap_or_else(|| "?".into());
            format!("{}: {st}", lang.t("Status setzen", "Set status"))
        }
        ActionParams::StartSession(c) => if c.claude { lang.t("Claude-Sitzung starten", "Start Claude session") } else { lang.t("Sitzung starten", "Start session") }.to_string(),
    }
}

enum Act {
    New(ActionKind),
    Edit(i64),
    Duplicate(i64),
    Delete(i64),
    Move(i64, i32),
}

impl PlanUi {
    fn qerr(&mut self, cx: &Cx, e: QuickError) {
        self.msg = Some(Msg { text: e.message(cx.lang), ok: false, at: Instant::now() });
    }

    fn qrun<T>(&mut self, cx: &Cx, r: QRes<T>) -> Option<T> {
        match r {
            Ok(v) => {
                self.dirty = true;
                self.msg = None;
                self.quick_ed.stale = true;
                Some(v)
            }
            Err(e) => {
                self.qerr(cx, e);
                None
            }
        }
    }

    fn quick_ed_sync(&mut self, cx: &Cx, sum: &Rc<ProjectSummary>) {
        let types: Vec<&TypeSummary> = sum.types.iter().filter(|t| !t.ticket_type.system).collect();
        if !self.quick_ed.sel_type.is_some_and(|s| types.iter().any(|t| t.ticket_type.id == s)) {
            self.quick_ed.sel_type = types.first().map(|t| t.ticket_type.id);
            self.quick_ed.stale = true;
        }
        let fresh = self.quick_ed.loaded.as_ref().is_some_and(|(t, s)| Some(*t) == self.quick_ed.sel_type && Rc::ptr_eq(s, sum));
        if fresh && !self.quick_ed.stale {
            return;
        }
        self.quick_ed.stale = false;
        let Some(t) = self.quick_ed.sel_type else {
            self.quick_ed.list.clear();
            return;
        };
        self.quick_ed.loaded = Some((t, sum.clone()));
        self.quick_ed.list = list_quick_actions(cx.conn, t)
            .unwrap_or_default()
            .into_iter()
            .map(|a| {
                let problem = if a.invalid { Some(check_quick_action(cx.conn, a.id).err().map(|e| e.message(cx.lang)).unwrap_or_else(|| QuickError::ActionInvalid.message(cx.lang))) } else { None };
                Row { action: a, problem }
            })
            .collect();
    }

    pub(super) fn quick_tab(&mut self, ui: &mut Ui, cx: &Cx) {
        let lang = cx.lang;
        let Some(sum) = self.sum.clone() else { return };
        self.quick_ed_sync(cx, &sum);
        hint(ui, lang.t(
            "Schnellaktionen erscheinen im Schnellmenü (Taste „.“) der Tickets des gewählten Typs – zusätzlich zu den Feldern und den erlaubten Statuswechseln.",
            "Quick actions appear in the quick menu (key “.”) of tickets of the selected type – in addition to the fields and the allowed status changes.",
        ));
        ui.add_space(S2);
        let types: Vec<&TypeSummary> = sum.types.iter().filter(|t| !t.ticket_type.system).collect();
        if types.is_empty() {
            widgets::card(ui, |ui| {
                ui.label(RichText::new(lang.t("Dieses Projekt hat noch keine Typen.", "This project has no types yet.")).color(style::dim(ui)));
            });
            return;
        }
        let mut pick: Option<i64> = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
            for t in &types {
                let tt = &t.ticket_type;
                if style::pill(ui, &tt.display(lang), None, color_of(&tt.color), self.quick_ed.sel_type == Some(tt.id)).clicked() {
                    pick = Some(tt.id);
                }
            }
        });
        if let Some(p) = pick {
            self.quick_ed.sel_type = Some(p);
            self.quick_ed.stale = true;
            self.quick_ed.confirm_delete = None;
            self.quick_ed_sync(cx, &sum);
        }
        ui.add_space(S2);
        let Some(type_id) = self.quick_ed.sel_type else { return };
        let mut acts: Vec<Act> = Vec::new();
        let order: Vec<i64> = self.quick_ed.list.iter().map(|r| r.action.id).collect();
        widgets::card(ui, |ui| {
            if self.quick_ed.list.is_empty() {
                ui.label(RichText::new(lang.t("Dieser Typ hat noch keine Schnellaktionen.", "This type has no quick actions yet.")).color(style::dim(ui)));
            }
            for (i, r) in self.quick_ed.list.iter().enumerate() {
                let a = &r.action;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.label(RichText::new(format!("{}", i + 1)).small().color(style::dim(ui)));
                    ui.label(RichText::new(a.display(lang)).strong());
                    if a.key.is_some() {
                        ui.label(RichText::new(lang.t("Standard", "Default")).small().color(style::dim(ui))).on_hover_text(lang.t(
                            "Eingebaute Aktion: Der Name wird übersetzt, bis du ihn änderst.",
                            "Built-in action: the name is translated until you change it.",
                        ));
                    }
                    ui.label(RichText::new(describe(a, &sum, lang)).small().color(style::dim(ui)));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if self.quick_ed.confirm_delete == Some(a.id) {
                            if widgets::small_danger(ui, lang.t("Wirklich löschen", "Really delete")).clicked() {
                                acts.push(Act::Delete(a.id));
                            }
                            if ui.add(egui::Button::new(RichText::new(lang.t("Abbrechen", "Cancel")).small())).clicked() {
                                self.quick_ed.confirm_delete = None;
                            }
                        } else {
                            if style::icon_btn_small(ui, SIcon::Close, lang.t("Löschen", "Delete")).clicked() {
                                self.quick_ed.confirm_delete = Some(a.id);
                            }
                            if ui.add(egui::Button::new(RichText::new(lang.t("Duplizieren", "Duplicate")).small())).clicked() {
                                acts.push(Act::Duplicate(a.id));
                            }
                            if ui.add(egui::Button::new(RichText::new(lang.t("Bearbeiten", "Edit")).small())).clicked() {
                                acts.push(Act::Edit(a.id));
                            }
                            if style::icon_btn_small(ui, SIcon::Down, lang.t("Nach unten", "Move down")).clicked() {
                                acts.push(Act::Move(a.id, 1));
                            }
                            if style::icon_btn_small(ui, SIcon::Up, lang.t("Nach oben", "Move up")).clicked() {
                                acts.push(Act::Move(a.id, -1));
                            }
                        }
                    });
                });
                if let Some(p) = &r.problem {
                    ui.horizontal_wrapped(|ui| widgets::error(ui, &format!("{} {p}", lang.t("Ungültig:", "Invalid:"))));
                }
                ui.add_space(2.0);
            }
        });
        ui.add_space(S2);
        ui.horizontal(|ui| {
            ui.label(RichText::new(lang.t("Neue Aktion:", "New action:")).color(style::dim(ui)));
            for k in ActionKind::ALL {
                if ui.button(format!("+ {}", k.label(lang))).on_hover_text(kind_hint(k, lang)).clicked() {
                    acts.push(Act::New(k));
                }
            }
        });
        let _ = type_id;
        for a in acts {
            self.quick_ed_act(cx, &sum, type_id, &order, a);
        }
    }

    fn quick_ed_act(&mut self, cx: &Cx, sum: &ProjectSummary, type_id: i64, order: &[i64], a: Act) {
        let lang = cx.lang;
        match a {
            Act::New(kind) => {
                let first_target = sum.types.iter().find(|t| !t.ticket_type.system).map(|t| t.ticket_type.id).unwrap_or(0);
                let first_status = sum.type_by_id(type_id).and_then(|t| t.workflow_statuses.first().copied()).unwrap_or(0);
                self.quick_ed.dlg = Some(Dlg::new_action(type_id, kind, first_target, first_status));
            }
            Act::Edit(id) => {
                if let Some(r) = self.quick_ed.list.iter().find(|r| r.action.id == id) {
                    self.quick_ed.dlg = Some(Dlg::from_action(&r.action, lang));
                }
            }
            Act::Duplicate(id) => {
                let r = duplicate_quick_action(cx.conn, id, lang);
                self.qrun(cx, r);
            }
            Act::Delete(id) => {
                self.quick_ed.confirm_delete = None;
                let r = delete_quick_action(cx.conn, id);
                self.qrun(cx, r);
            }
            Act::Move(id, dir) => {
                let mut o = order.to_vec();
                if move_id(&mut o, id, dir) {
                    let r = reorder_quick_actions(cx.conn, type_id, &o);
                    self.qrun(cx, r);
                }
            }
        }
    }

    pub(super) fn quick_ed_dialogs(&mut self, ctx: &egui::Context, cx: &Cx) {
        let Some(mut d) = self.quick_ed.dlg.take() else { return };
        let Some(sum) = self.sum.clone() else { return };
        let lang = cx.lang;
        let problem = d.problem(cx.conn, lang);
        let mut save = false;
        let mut close = false;
        let links: Vec<LinkType> = self.links.iter().filter(|l| l.target_kind == TargetKind::Ticket).cloned().collect();
        let owner = sum.type_by_id(d.type_id).cloned();
        let m = egui::Modal::new(Self::modal_id("quick_ed")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 600.0));
            let title = match (d.id, lang) {
                (None, Lang::De) => format!("Neue Aktion: {}", d.kind.label(lang)),
                (None, Lang::En) => format!("New action: {}", d.kind.label(lang)),
                (Some(_), Lang::De) => format!("Aktion bearbeiten: {}", d.kind.label(lang)),
                (Some(_), Lang::En) => format!("Edit action: {}", d.kind.label(lang)),
            };
            widgets::dialog_title(ui, &title);
            egui::ScrollArea::vertical().max_height((ctx.content_rect().height() - 260.0).max(240.0)).auto_shrink([true, true]).show(ui, |ui| {
                hint(ui, kind_hint(d.kind, lang));
                ui.add_space(S1);
                ui.label(RichText::new(lang.t("Name (Text im Menü)", "Name (text in the menu)")).strong());
                if ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(f32::INFINITY).margin(egui::Margin::symmetric(8, 5))).changed() {
                    d.touched = true;
                }
                if d.default_name.is_some() {
                    hint(ui, lang.t("Standardname – er wird automatisch übersetzt, bis du ihn änderst.", "Default name – it is translated automatically until you change it."));
                }
                ui.add_space(S2);
                match d.kind {
                    ActionKind::CreateTicket => create_form(ui, &mut d, &sum, &links, lang, cx),
                    ActionKind::ChangeStatus => change_form(ui, &mut d, &sum, owner.as_ref(), lang, cx),
                    ActionKind::StartSession => session_form(ui, &mut d, lang),
                }
            });
            ui.add_space(S2);
            d.drop_stale_err();
            if let Some(e) = d.err.as_ref() {
                ui.horizontal_wrapped(|ui| widgets::error(ui, e));
                ui.add_space(S1);
            } else if let Some(p) = problem.as_ref() {
                // ein noch leerer Name ist beim Öffnen kein Fehler, sondern ein Hinweis (das Speichern bleibt gesperrt)
                if d.name_missing() && !d.touched {
                    hint(ui, p);
                } else {
                    ui.horizontal_wrapped(|ui| widgets::error(ui, p));
                }
                ui.add_space(S1);
            }
            ui.horizontal(|ui| {
                if widgets::primary_enabled(ui, problem.is_none(), lang.t("Speichern", "Save")).clicked() {
                    save = true;
                }
                if widgets::secondary(ui, lang.t("Abbrechen", "Cancel")).clicked() {
                    close = true;
                }
            });
        });
        if m.should_close() {
            close = true;
        }
        if save && problem.is_none() {
            let params = d.params();
            let res = match d.id {
                Some(id) => update_quick_action(cx.conn, id, &UpdateQuickAction { name: d.name_for_update(), params: Some(params) }),
                None => create_quick_action(cx.conn, &NewQuickAction::new(d.type_id, d.name.trim(), params)).map(|_| ()),
            };
            match res {
                Ok(()) => {
                    self.dirty = true;
                    self.quick_ed.stale = true;
                    self.msg = None;
                    return;
                }
                Err(e) => d.set_err(e.message(lang)),
            }
        }
        if !close {
            self.quick_ed.dlg = Some(d);
        }
    }
}

// ------------------------------------------------------------------ Formulare je Aktionsart

fn type_combo(ui: &mut Ui, salt: &str, sum: &ProjectSummary, cur: &mut i64, lang: Lang) {
    let name = sum.type_by_id(*cur).map(|t| t.ticket_type.display(lang)).unwrap_or_else(|| lang.t("(wählen)", "(choose)").to_string());
    egui::ComboBox::from_id_salt(salt).selected_text(name).width(240.0).show_ui(ui, |ui| {
        for t in sum.types.iter().filter(|t| !t.ticket_type.system) {
            ui.selectable_value(cur, t.ticket_type.id, t.ticket_type.display(lang));
        }
    });
}

fn create_form(ui: &mut Ui, d: &mut Dlg, sum: &ProjectSummary, links: &[LinkType], lang: Lang, cx: &Cx) {
    ui.label(RichText::new(lang.t("Ziel-Typ des neuen Tickets", "Type of the new ticket")).strong());
    let before = d.target_type;
    type_combo(ui, "quick_ed_target", sum, &mut d.target_type, lang);
    if before != d.target_type {
        d.entries.clear();
    }
    ui.add_space(S2);
    ui.label(RichText::new(lang.t("Verknüpfung mit dem Ausgangsticket", "Link to the source ticket")).strong());
    let cur = d.relation.as_ref().and_then(|r| links.iter().find(|l| l.id == r.link_type_id));
    let cur_name = cur.map(|l| l.names(lang).0).unwrap_or_else(|| lang.t("(keine Verknüpfung)", "(no link)").to_string());
    let mut chosen: Option<Option<i64>> = None;
    egui::ComboBox::from_id_salt("quick_ed_rel").selected_text(cur_name).width(240.0).show_ui(ui, |ui| {
        if ui.selectable_label(d.relation.is_none(), lang.t("(keine Verknüpfung)", "(no link)")).clicked() {
            chosen = Some(None);
        }
        for l in links {
            if ui.selectable_label(cur.is_some_and(|c| c.id == l.id), l.names(lang).0).clicked() {
                chosen = Some(Some(l.id));
            }
        }
    });
    match chosen {
        Some(None) => d.relation = None,
        Some(Some(id)) => d.relation = Some(RelationSpec { link_type_id: id, direction: d.relation.as_ref().map(|r| r.direction).unwrap_or(LinkDirection::Out) }),
        None => {}
    }
    if let (Some(rel), Some(l)) = (d.relation.as_mut(), cur) {
        if !l.symmetric {
            let name = l.names(lang).0;
            let out = match lang {
                Lang::De => format!("Dieses Ticket {name} das neue Ticket"),
                Lang::En => format!("This ticket {name} the new ticket"),
            };
            let inn = match lang {
                Lang::De => format!("Das neue Ticket {name} dieses Ticket"),
                Lang::En => format!("The new ticket {name} this ticket"),
            };
            widgets::radio(ui, &mut rel.direction, LinkDirection::Out, out);
            widgets::radio(ui, &mut rel.direction, LinkDirection::In, inn);
        }
    }
    ui.add_space(S2);
    ui.label(RichText::new(lang.t("Standardwerte des neuen Tickets", "Default values of the new ticket")).strong());
    let fields: Vec<Field> = sum.type_by_id(d.target_type).map(|t| t.fields.clone()).unwrap_or_default();
    entry_rows(ui, d, &fields, true, sum, lang, cx);
    hint(ui, &format!("{} {}", lang.t("Platzhalter in Texten:", "Placeholders in texts:"), PLACEHOLDERS.iter().map(|p| format!("{{{{{p}}}}}")).collect::<Vec<_>>().join(" ")));
    hint(ui, lang.t("Ohne Titel fragt das Menü beim Ausführen nach dem Titel.", "Without a title the menu asks for it when the action runs."));
}

fn change_form(ui: &mut Ui, d: &mut Dlg, sum: &ProjectSummary, owner: Option<&TypeSummary>, lang: Lang, cx: &Cx) {
    let Some(owner) = owner else { return };
    if !owner.ticket_type.workflow {
        widgets::error(ui, &crate::planning::PlanError::WorkflowOff.message(lang));
        return;
    }
    ui.label(RichText::new(lang.t("Zielstatus", "Target status")).strong());
    let name = sum.status(d.to_status).map(|s| s.display(lang)).unwrap_or_else(|| lang.t("(wählen)", "(choose)").to_string());
    egui::ComboBox::from_id_salt("quick_ed_status").selected_text(name).width(240.0).show_ui(ui, |ui| {
        for sid in &owner.workflow_statuses {
            if let Some(s) = sum.status(*sid) {
                ui.selectable_value(&mut d.to_status, s.id, s.display(lang));
            }
        }
    });
    ui.add_space(S2);
    ui.label(RichText::new(lang.t("Zusätzlich setzen", "Also set")).strong());
    entry_rows(ui, d, &owner.fields, false, sum, lang, cx);
}

fn session_form(ui: &mut Ui, d: &mut Dlg, lang: Lang) {
    ui.label(RichText::new(lang.t("Shell-Kennung (leer = Standard-Shell)", "Shell id (empty = default shell)")).strong());
    ui.add(egui::TextEdit::singleline(&mut d.shell).desired_width(240.0).hint_text("pwsh").margin(egui::Margin::symmetric(8, 5)));
    ui.add_space(S1);
    widgets::check(ui, &mut d.claude, lang.t("Claude in der Sitzung starten", "Start Claude in the session"));
    if !d.claude {
        d.prompt = false;
    }
    ui.add_enabled_ui(d.claude, |ui| {
        widgets::check(ui, &mut d.prompt, lang.t("Titel und Beschreibung des Tickets als Prompt übergeben", "Pass the ticket's title and description as the prompt"));
    });
    widgets::check(ui, &mut d.cwd, lang.t("Arbeitsordner des Projekts als Startordner", "Use the project's working folder as the start folder"));
}

/// Liste der Einträge (Feld + Wert) mit Hinzufügen/Entfernen.
fn entry_rows(ui: &mut Ui, d: &mut Dlg, fields: &[Field], allow_title: bool, sum: &ProjectSummary, lang: Lang, cx: &Cx) {
    let mut remove: Option<usize> = None;
    let used: Vec<FieldRef> = d.entries.iter().map(|e| e.0).collect();
    for i in 0..d.entries.len() {
        ui.horizontal(|ui| {
            let cur = d.entries[i].0;
            let name = field_name(cur, fields, lang);
            egui::ComboBox::from_id_salt(("quick_ed_field", i)).selected_text(name).width(170.0).show_ui(ui, |ui| {
                let mut pick: Option<FieldRef> = None;
                if allow_title && (cur == FieldRef::Title || !used.contains(&FieldRef::Title)) && ui.selectable_label(cur == FieldRef::Title, lang.t("Titel", "Title")).clicked() {
                    pick = Some(FieldRef::Title);
                }
                for f in fields.iter().filter(|f| f.kind != FieldKind::Terminal) {
                    let r = FieldRef::Field(f.id);
                    if (cur == r || !used.contains(&r)) && ui.selectable_label(cur == r, f.display(lang)).clicked() {
                        pick = Some(r);
                    }
                }
                if let Some(p) = pick.filter(|p| *p != cur) {
                    d.entries[i] = (p, match p {
                        FieldRef::Title => json!(""),
                        FieldRef::Field(id) => fields.iter().find(|f| f.id == id).map(start_value).unwrap_or(Value::Null),
                    });
                }
            });
            let (fr, v) = &mut d.entries[i];
            match fr {
                FieldRef::Title => {
                    let mut s = v.as_str().unwrap_or("").to_string();
                    if ui.add(egui::TextEdit::singleline(&mut s).desired_width(260.0).hint_text("{{source.title}}").margin(egui::Margin::symmetric(8, 4))).changed() {
                        *v = Value::String(s);
                    }
                }
                FieldRef::Field(id) => {
                    if let Some(f) = fields.iter().find(|f| f.id == *id) {
                        value_editor(ui, ("quick_ed_val", i), f, v, sum, lang, cx);
                    }
                }
            }
            if style::icon_btn_small(ui, SIcon::Close, lang.t("Entfernen", "Remove")).clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        d.entries.remove(i);
    }
    if ui.add(egui::Button::new(format!("+ {}", lang.t("Wert hinzufügen", "Add a value"))).small()).clicked() {
        d.add_entry(fields, allow_title);
    }
}

fn field_name(r: FieldRef, fields: &[Field], lang: Lang) -> String {
    match r {
        FieldRef::Title => lang.t("Titel", "Title").to_string(),
        FieldRef::Field(id) => fields.iter().find(|f| f.id == id).map(|f| f.display(lang)).unwrap_or_else(|| format!("#{id}")),
    }
}

/// Wert eines Feldes: Ticket-Verweis = „das Ausgangsticket“, Version/Meilenstein aus der Liste, sonst der Standard-Editor.
fn value_editor(ui: &mut Ui, salt: (&str, usize), f: &Field, v: &mut Value, sum: &ProjectSummary, lang: Lang, cx: &Cx) {
    let id = Id::new(salt);
    match f.kind {
        FieldKind::Ticket => {
            let mut on = v.as_str() == Some(SOURCE_REF);
            if widgets::check(ui, &mut on, lang.t("das Ausgangsticket", "the source ticket")).changed() {
                *v = if on { json!(SOURCE_REF) } else { Value::Null };
            }
        }
        FieldKind::Version | FieldKind::Milestone => {
            let kind = if f.kind == FieldKind::Version { ReleaseKind::Version } else { ReleaseKind::Milestone };
            let list = list_releases(cx.conn, kind, sum.project.id).unwrap_or_default();
            let cur = v.as_i64();
            let name = cur.and_then(|c| list.iter().find(|r| r.id == c)).map(|r| r.name.clone()).unwrap_or_else(|| lang.t("(wählen)", "(choose)").to_string());
            egui::ComboBox::from_id_salt(id).selected_text(name).width(180.0).show_ui(ui, |ui| {
                for r in &list {
                    if ui.selectable_label(cur == Some(r.id), &r.name).clicked() {
                        *v = json!(r.id);
                    }
                }
            });
        }
        k => {
            full_editor(ui, id, k, &f.options, v, lang);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::quick::actions::fixture::*;
    use crate::planning::summary::project_summary;

    fn dlg(f: &Fx, kind: ActionKind) -> Dlg {
        Dlg::new_action(f.story, kind, f.dev, f.progress)
    }

    #[test]
    fn dialog_builds_params_for_every_kind() {
        let f = fx();
        let mut d = dlg(&f, ActionKind::CreateTicket);
        d.entries = vec![(FieldRef::Title, json!("Task {{source.key}}"))];
        d.relation = Some(RelationSpec { link_type_id: 1, direction: LinkDirection::In });
        let ActionParams::CreateTicket(c) = d.params() else { panic!() };
        assert_eq!((c.ticket_type_id, c.defaults.len(), c.relation.is_some()), (f.dev, 1, true));
        let mut d = dlg(&f, ActionKind::ChangeStatus);
        let sp = f.field(f.story, "story_points");
        d.entries = vec![(FieldRef::Field(sp.id), json!(3)), (FieldRef::Title, json!("ignored"))];
        let ActionParams::ChangeStatus(c) = d.params() else { panic!() };
        assert_eq!((c.to_status_id, c.set_values.len()), (f.progress, 1), "„Titel“ gibt es nur beim Anlegen");
        let mut d = dlg(&f, ActionKind::StartSession);
        d.shell = "  pwsh ".into();
        d.claude = true;
        let ActionParams::StartSession(c) = d.params() else { panic!() };
        assert_eq!(c.shell.as_deref(), Some("pwsh"));
        d.shell.clear();
        let ActionParams::StartSession(c) = d.params() else { panic!() };
        assert_eq!(c.shell, None);
    }

    #[test]
    fn live_validation_uses_the_texts_of_the_data_layer_in_both_languages() {
        let f = fx();
        let mut d = dlg(&f, ActionKind::CreateTicket);
        assert!(d.problem(&f.c, Lang::En).unwrap().contains("name"), "ohne Namen");
        d.name = "Task".into();
        assert_eq!(d.problem(&f.c, Lang::En), None);
        d.entries = vec![(FieldRef::Title, json!("{{nope}}"))];
        assert!(d.problem(&f.c, Lang::En).unwrap().contains("placeholder"));
        assert!(d.problem(&f.c, Lang::De).unwrap().contains("Platzhalter"));
        d.entries = vec![(FieldRef::Title, json!("ok"))];
        d.target_type = 99_999;
        assert!(d.problem(&f.c, Lang::De).unwrap().contains("Ziel-Typ"));
        let mut c = dlg(&f, ActionKind::StartSession);
        c.name = "S".into();
        c.prompt = true;
        assert!(c.problem(&f.c, Lang::En).unwrap().contains("Claude"), "Prompt braucht Claude");
        c.claude = true;
        assert_eq!(c.problem(&f.c, Lang::En), None);
    }

    #[test]
    fn an_empty_name_is_a_hint_until_edited_and_a_stale_save_error_disappears_after_a_change() {
        let f = fx();
        let mut d = dlg(&f, ActionKind::ChangeStatus);
        // frisch geöffnet: Name fehlt, aber noch nicht bearbeitet (die Oberfläche zeigt dann nur einen Hinweis)
        assert!(d.name_missing() && !d.touched && d.problem(&f.c, Lang::En).is_some());
        d.name = "Done".into();
        assert!(!d.name_missing());
        // Fehler des Speicherversuchs bleibt, solange nichts geändert wird, und verschwindet nach der ersten Änderung
        d.set_err("Boom".into());
        d.drop_stale_err();
        assert_eq!(d.err.as_deref(), Some("Boom"));
        d.name.push('!');
        d.drop_stale_err();
        assert_eq!(d.err, None);
        d.set_err("Boom".into());
        d.to_status += 1;
        d.drop_stale_err();
        assert_eq!(d.err, None, "auch eine geänderte Auswahl macht den Fehler überholt");
    }

    #[test]
    fn save_roundtrip_edit_duplicate_move_and_delete_through_the_dialog() {
        let f = fx();
        let mut d = dlg(&f, ActionKind::ChangeStatus);
        d.name = "Finish".into();
        let id = create_quick_action(&f.c, &NewQuickAction::new(d.type_id, d.name.trim(), d.params())).unwrap();
        let a = crate::planning::quick::actions::get_quick_action(&f.c, id).unwrap();
        let mut e = Dlg::from_action(&a, Lang::En);
        assert_eq!((e.name.as_str(), e.to_status, e.kind), ("Finish", f.progress, ActionKind::ChangeStatus));
        e.to_status = f.done;
        e.name = "Finish it".into();
        update_quick_action(&f.c, id, &UpdateQuickAction { name: e.name_for_update(), params: Some(e.params()) }).unwrap();
        let a = crate::planning::quick::actions::get_quick_action(&f.c, id).unwrap();
        assert_eq!((a.display(Lang::En), a.kind()), ("Finish it".to_string(), ActionKind::ChangeStatus));
        let copy = duplicate_quick_action(&f.c, id, Lang::En).unwrap();
        assert_eq!(list_quick_actions(&f.c, f.story).unwrap().iter().map(|a| a.id).collect::<Vec<_>>(), vec![id, copy]);
        let mut o = vec![id, copy];
        assert!(move_id(&mut o, id, 1));
        reorder_quick_actions(&f.c, f.story, &o).unwrap();
        assert_eq!(list_quick_actions(&f.c, f.story).unwrap()[0].id, copy);
        delete_quick_action(&f.c, copy).unwrap();
        assert_eq!(list_quick_actions(&f.c, f.story).unwrap().len(), 1);
    }

    #[test]
    fn builtin_names_stay_default_until_changed() {
        let f = fx();
        let mut n = NewQuickAction::new(f.story, "x", ActionParams::StartSession(StartSessionParams::default()));
        n.key = Some("work".into());
        n.name = None;
        let id = create_quick_action(&f.c, &n).unwrap();
        let a = crate::planning::quick::actions::get_quick_action(&f.c, id).unwrap();
        let mut d = Dlg::from_action(&a, Lang::En);
        assert!(d.has_key && d.default_name.is_some());
        assert_eq!(d.name_for_update(), None, "Name unverändert = Standard bleibt");
        d.name = "My work".into();
        assert_eq!(d.name_for_update(), Some("My work".into()));
        assert_eq!(d.problem(&f.c, Lang::En), None);
        d.name.clear();
        assert_eq!(d.problem(&f.c, Lang::En), None, "Aktionen mit Schlüssel dürfen den Namen leeren (Standardname)");
    }

    #[test]
    fn add_entry_picks_free_fields_and_start_values_fit_the_kind() {
        let f = fx();
        let sum = project_summary(&f.c, f.project).unwrap();
        let fields = sum.type_by_id(f.dev).unwrap().fields.clone();
        let mut d = dlg(&f, ActionKind::CreateTicket);
        assert!(d.add_entry(&fields, true));
        assert_eq!(d.entries[0].0, FieldRef::Title);
        let mut n = 1;
        while d.add_entry(&fields, true) {
            n += 1;
            assert!(n < 50);
        }
        assert_eq!(d.entries.len(), 1 + fields.len(), "Titel plus jedes Feld genau einmal");
        let mut keys: Vec<FieldRef> = d.entries.iter().map(|e| e.0).collect();
        keys.dedup();
        assert_eq!(keys.len(), d.entries.len());
        // Ticket-Feld startet als „Ausgangsticket“, Version leer
        let ticket_field = sum.type_by_id(f.story).unwrap().field_by_key("epic").unwrap();
        assert_eq!(start_value(ticket_field), json!(SOURCE_REF));
        let version = sum.type_by_id(f.story).unwrap().field_by_key("release_version").unwrap();
        assert_eq!(start_value(version), Value::Null);
        // ohne Titel-Erlaubnis kein Titel
        let mut e = dlg(&f, ActionKind::ChangeStatus);
        e.add_entry(&sum.type_by_id(f.story).unwrap().fields, false);
        assert!(matches!(e.entries[0].0, FieldRef::Field(_)));
    }

    #[test]
    fn descriptions_and_hints_exist_in_both_languages() {
        let f = fx();
        let sum = project_summary(&f.c, f.project).unwrap();
        for k in ActionKind::ALL {
            assert_ne!(kind_hint(k, Lang::De), kind_hint(k, Lang::En));
        }
        let d = {
            let mut d = dlg(&f, ActionKind::CreateTicket);
            d.name = "A".into();
            d
        };
        let id = create_quick_action(&f.c, &NewQuickAction::new(d.type_id, "A", d.params())).unwrap();
        let a = crate::planning::quick::actions::get_quick_action(&f.c, id).unwrap();
        assert!(describe(&a, &sum, Lang::De).starts_with("Ticket anlegen"));
        assert!(describe(&a, &sum, Lang::En).starts_with("Create ticket"));
    }
}
