//! Typgerechte Eingabe der Feldwerte eines Tickets: Text, Rich-Text (Text|Visuell), Haken, Auswahl mit Farben, Zahl mit
//! Einheit und Grenzen, Datum, Mehrfachauswahl, Labels mit freiem Anlegen und der Ticket-Verweis mit Suchfeld.
//! Dieselben Bausteine nutzen der Anlege-Dialog, die Ticket-Ansicht, der Pflichtfeld-Dialog beim Statuswechsel, der
//! Typwechsel und „In Ticket umwandeln“. Gespeichert wird nicht hier, sondern vom Aufrufer anhand des [`Event`].

use super::helpers::color_of;
use super::ref_inputs::ref_value_editor;
use super::ticket_data::search_tickets;
use super::ticket_draft::{field_hint, Draft};
use super::widgets;
use crate::markdown::{is_safe_url, rich_text_field_ex, FieldOpts};
use crate::planning::fields::{Field, FieldKind};
use crate::planning::tickets::{get_ticket, Ticket};
use crate::planning::types::get_type;
use crate::state::Lang;
use crate::style::{self, S1, S2};
use eframe::egui::{self, Color32, Id, Key, Modifiers, Pos2, Rect, Response, RichText, Sense, Stroke, Ui, Vec2};
use rusqlite::Connection;
use serde_json::Value;

/// Umgebung einer Eingabe.
pub(super) struct InCx<'a> {
    pub conn: &'a Connection,
    pub lang: Lang,
    /// Projekt des Tickets (Verweise bleiben im Projekt).
    pub project_id: i64,
    /// Das Ticket selbst (kann nicht auf sich verweisen); `None` beim Anlegen.
    pub ticket_id: Option<i64>,
    /// Projektschlüssel, die in Markdown als Ticket-Verweis gelten.
    pub prefixes: &'a [String],
    pub labels: &'a [String],
    /// Eindeutige Grundlage für Widget-Kennungen (je Dialog/Ticket verschieden).
    pub id: Id,
}

/// Was bei einer Eingabe passiert ist.
#[derive(Default, Debug)]
pub(super) struct Event {
    pub changed: bool,
    /// Der Wert soll gespeichert werden (Fokus verlassen, Auswahl getroffen, „Speichern“).
    pub commit: bool,
    /// Die Eingabe soll verworfen und neu aus dem gespeicherten Wert gelesen werden.
    pub revert: bool,
    /// Enter in einem einzeiligen Feld (der Anlege-Dialog speichert dann).
    pub enter: bool,
    /// Angeklickter Ticket-Verweis im Markdown (`PAN-12`).
    pub open_key: Option<String>,
    /// Angeklickter Terminal-Chip einer laufenden Sitzung (Sprung zur Sitzung).
    pub open_session: Option<u64>,
}

/// Adresse im Browser öffnen (nur http/https/mailto).
pub(super) fn open_url(url: &str) {
    if is_safe_url(url) {
        let _ = std::process::Command::new("explorer").arg(url).spawn();
    }
}

// ------------------------------------------------------------------ kleine Bausteine

/// Zeile mit Farbpunkt und Text (Einträge von Auswahllisten).
pub(super) fn dot_item(ui: &mut Ui, color: Color32, text: &str, selected: bool) -> Response {
    let g = ui.painter().layout_no_wrap(text.to_string(), egui::TextStyle::Button.resolve(ui.style()), ui.visuals().text_color());
    let w = ui.available_width().max(g.size().x + 34.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 24.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 5.0, ui.visuals().widgets.hovered.bg_fill);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    } else if selected {
        ui.painter().rect_filled(rect, 5.0, style::accent().gamma_multiply(0.2));
    }
    ui.painter().circle_filled(Pos2::new(rect.left() + 11.0, rect.center().y), 4.5, color);
    ui.painter().galley(Pos2::new(rect.left() + 24.0, rect.center().y - g.size().y / 2.0), g, ui.visuals().text_color());
    resp
}

/// Farbige Marke mit Text und Schließen-Kreuz; `true`, wenn das Kreuz geklickt wurde.
pub(super) fn chip_removable(ui: &mut Ui, text: &str, color: Color32, tip: &str) -> bool {
    let g = ui.painter().layout_no_wrap(text.to_string(), egui::FontId::proportional(12.5), color);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(g.size().x + 32.0, 22.0), Sense::click());
    ui.painter().rect_filled(rect, 11.0, color.gamma_multiply(0.2));
    ui.painter().galley(Pos2::new(rect.left() + 9.0, rect.center().y - g.size().y / 2.0), g, color);
    let c = Pos2::new(rect.right() - 11.0, rect.center().y);
    let hot = resp.hovered() && ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| p.x > rect.right() - 20.0);
    let col = if hot { Color32::WHITE } else { color };
    let s = Stroke::new(1.3_f32, col);
    ui.painter().line_segment([c + Vec2::new(-3.0, -3.0), c + Vec2::new(3.0, 3.0)], s);
    ui.painter().line_segment([c + Vec2::new(-3.0, 3.0), c + Vec2::new(3.0, -3.0)], s);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let _ = resp.clone().on_hover_text(tip);
    resp.clicked() && ui.input(|i| i.pointer.interact_pos()).is_some_and(|p| p.x > rect.right() - 22.0)
}

fn single_line(ui: &mut Ui, id: Id, text: &mut String, width: f32, hint: &str, limit: Option<usize>, d_edited: &mut bool, ev: &mut Event) -> Response {
    let mut te = egui::TextEdit::singleline(text).id(id).desired_width(width).margin(egui::Margin::symmetric(8, 4));
    if !hint.is_empty() {
        te = te.hint_text(hint);
    }
    if let Some(l) = limit {
        te = te.char_limit(l);
    }
    let r = ui.add_sized([width, 26.0], te);
    if r.changed() {
        *d_edited = true;
        ev.changed = true;
    }
    if r.lost_focus() {
        let (esc, enter) = ui.input(|i| (i.key_pressed(Key::Escape), i.key_pressed(Key::Enter)));
        if esc {
            ev.revert = true;
        } else {
            if *d_edited {
                ev.commit = true;
            }
            ev.enter |= enter;
        }
    }
    r
}

// ------------------------------------------------------------------ Ticket-Suche

/// Kleine Farbmarke und Schlüssel für ein Ticket (Auswahl- und Ergebniszeilen).
fn ticket_line(ui: &mut Ui, conn: &Connection, t: &Ticket, width: f32, selected: bool) -> Response {
    let (icon, color) = get_type(conn, t.type_id).map(|ty| (ty.icon, color_of(&ty.color))).unwrap_or((None, Color32::GRAY));
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 26.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 5.0, ui.visuals().widgets.hovered.bg_fill);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    } else if selected {
        ui.painter().rect_filled(rect, 5.0, style::accent().gamma_multiply(0.2));
    }
    widgets::paint_type_badge(ui.painter(), Rect::from_min_size(Pos2::new(rect.left() + 4.0, rect.center().y - 9.0), Vec2::splat(18.0)), icon.as_deref(), color);
    let key = ui.painter().layout_no_wrap(t.key(), egui::FontId::monospace(12.5), style::dim(ui));
    let kw = key.size().x;
    ui.painter().galley(Pos2::new(rect.left() + 28.0, rect.center().y - key.size().y / 2.0), key, style::dim(ui));
    let mut job = egui::text::LayoutJob::simple_singleline(t.title.clone(), egui::TextStyle::Body.resolve(ui.style()), ui.visuals().text_color());
    job.wrap.max_width = (width - 28.0 - kw - 14.0).max(20.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    let g = ui.painter().layout_job(job);
    ui.painter().galley(Pos2::new(rect.left() + 28.0 + kw + 8.0, rect.center().y - g.size().y / 2.0), g, ui.visuals().text_color());
    resp
}

/// Suchfeld für Tickets (Schlüssel oder Titel). Zeigt beim Tippen (und solange das Feld offen ist) die Treffer darunter und
/// liefert das gewählte Ticket. `scope`: Projekt (`None` = alle), `types`: erlaubte Typen (leer = alle), `exclude`: nie anbieten.
pub(super) fn ticket_search(ui: &mut Ui, ic: &InCx, salt: Id, text: &mut String, scope: Option<i64>, types: &[i64], exclude: &[i64], hint: &str) -> Option<Ticket> {
    let width = ui.available_width().clamp(200.0, 420.0);
    let r = ui.add_sized([width, 26.0], egui::TextEdit::singleline(text).id(salt).hint_text(hint).margin(egui::Margin::symmetric(8, 4)));
    let open_id = salt.with("open");
    let rect_id = salt.with("rect");
    let was_open: bool = ui.data(|d| d.get_temp(open_id)).unwrap_or(false);
    let mut open = r.has_focus() || was_open;
    let mut picked = None;
    if open {
        let hits = search_tickets(ic.conn, scope, text, types, exclude, 8);
        let mut rect = r.rect;
        egui::Frame::NONE
            .fill(ui.visuals().extreme_bg_color)
            .stroke(Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color))
            .corner_radius(6.0)
            .inner_margin(egui::Margin::same(3))
            .show(ui, |ui| {
                ui.set_width(width - 8.0);
                if hits.is_empty() {
                    ui.label(RichText::new(ic.lang.t("Keine Treffer.", "No matches.")).small().color(style::dim(ui)));
                }
                for t in &hits {
                    if ticket_line(ui, ic.conn, t, width - 8.0, false).clicked() {
                        picked = Some(t.clone());
                    }
                }
                rect = rect.union(ui.min_rect());
            });
        ui.data_mut(|d| d.insert_temp(rect_id, rect));
        let prev: Rect = ui.data(|d| d.get_temp(rect_id)).unwrap_or(rect);
        if ui.input(|i| i.pointer.any_pressed()) && ui.input(|i| i.pointer.interact_pos()).is_some_and(|p| !prev.contains(p) && !r.rect.contains(p)) {
            open = false;
        }
        if ui.input(|i| i.key_pressed(Key::Escape)) && r.has_focus() {
            open = false;
        }
    }
    if picked.is_some() {
        open = false;
        text.clear();
    }
    ui.data_mut(|d| d.insert_temp(open_id, open));
    picked
}

/// Das gewählte Ticket als Zeile mit „entfernen“; `true`, wenn es entfernt werden soll. `open` = Schlüssel angeklickt.
fn ticket_chip(ui: &mut Ui, ic: &InCx, id: i64, ev: &mut Event, clearable: bool) -> bool {
    let mut clear = false;
    ui.horizontal(|ui| match get_ticket(ic.conn, id) {
        Ok(t) => {
            let w = ui.available_width().clamp(160.0, 420.0) - if clearable { 34.0 } else { 0.0 };
            if ticket_line(ui, ic.conn, &t, w, true).on_hover_text(ic.lang.t("Ticket öffnen", "Open ticket")).clicked() {
                ev.open_key = Some(t.key());
            }
            if clearable && style::icon_btn_small(ui, style::Icon::Close, ic.lang.t("Verweis entfernen", "Remove the reference")).clicked() {
                clear = true;
            }
        }
        Err(_) => {
            ui.label(RichText::new(format!("#{id} ({})", ic.lang.t("nicht mehr vorhanden", "no longer exists"))).color(widgets::DANGER));
            if clearable && style::icon_btn_small(ui, style::Icon::Close, ic.lang.t("Verweis entfernen", "Remove the reference")).clicked() {
                clear = true;
            }
        }
    });
    clear
}

// ------------------------------------------------------------------ die Eingabe je Feldtyp

/// Eingabe für ein Feld. `explicit_save`: Rich-Text zeigt „Speichern/Verwerfen“ (Ticket-Ansicht), sonst wird nur der Puffer
/// verändert (Dialoge).
pub(super) fn field_input(ui: &mut Ui, ic: &InCx, f: &Field, d: &mut Draft, explicit_save: bool) -> Event {
    let salt = ic.id.with(("field", f.id));
    let lang = ic.lang;
    let mut ev = Event::default();
    match f.kind {
        FieldKind::Text => {
            let limit = f.options.max_length.map(|m| m as usize);
            let w = ui.available_width().clamp(200.0, 560.0);
            let hint = field_hint(f, lang);
            let mut edited = d.edited;
            let mut text = std::mem::take(&mut d.text);
            single_line(ui, salt, &mut text, w, &hint, limit, &mut edited, &mut ev);
            d.text = text;
            d.edited = edited;
        }
        FieldKind::Number => {
            ui.horizontal(|ui| {
                let mut edited = d.edited;
                let mut text = std::mem::take(&mut d.text);
                single_line(ui, salt, &mut text, 130.0, "", None, &mut edited, &mut ev);
                d.text = text;
                d.edited = edited;
                let unit = f.options.unit.as_deref().filter(|u| !u.is_empty());
                if let Some(u) = unit {
                    ui.label(u);
                }
                let h = field_hint(f, lang);
                if !h.is_empty() && Some(h.as_str()) != unit {
                    ui.label(RichText::new(h).small().color(style::dim(ui)));
                }
            });
        }
        FieldKind::Date | FieldKind::Datetime => {
            ui.horizontal(|ui| {
                let mut edited = d.edited;
                let mut text = std::mem::take(&mut d.text);
                single_line(ui, salt, &mut text, 170.0, &field_hint(f, lang), None, &mut edited, &mut ev);
                d.text = text;
                d.edited = edited;
                let (label, tip, fmt) = if f.kind == FieldKind::Date {
                    (lang.t("Heute", "Today"), lang.t("Heutiges Datum eintragen", "Enter today's date"), "%Y-%m-%d")
                } else {
                    (lang.t("Jetzt", "Now"), lang.t("Aktuelle Uhrzeit eintragen", "Enter the current time"), "%Y-%m-%d %H:%M")
                };
                if ui.small_button(label).on_hover_text(tip).clicked() {
                    d.text = chrono::Local::now().format(fmt).to_string();
                    d.edited = true;
                    ev.changed = true;
                    ev.commit = true;
                }
                if !f.required && !d.text.is_empty() && ui.small_button(lang.t("Leeren", "Clear")).clicked() {
                    d.text.clear();
                    d.edited = true;
                    ev.changed = true;
                    ev.commit = true;
                }
            });
        }
        FieldKind::Checkbox => {
            let mut on = d.value.as_bool().unwrap_or(false);
            if widgets::check(ui, &mut on, "").changed() {
                d.value = Value::Bool(on);
                d.edited = true;
                ev.changed = true;
                ev.commit = true;
            }
        }
        FieldKind::Dropdown => {
            let cur = d.value.as_i64();
            let sel = cur.and_then(|id| f.options.values.iter().find(|v| v.id == id));
            ui.horizontal(|ui| {
                if let Some(v) = sel {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 5.0, v.color.as_deref().map(color_of).unwrap_or(Color32::GRAY));
                }
                let text = sel.map(|v| v.display(lang)).unwrap_or_else(|| "–".to_string());
                egui::ComboBox::from_id_salt(salt).selected_text(text).width(200.0).show_ui(ui, |ui| {
                    if f.options.values.is_empty() {
                        ui.label(lang.t("(keine Werte definiert)", "(no values defined)"));
                    }
                    if !f.required && ui.selectable_label(cur.is_none(), "–").clicked() {
                        d.value = Value::Null;
                        d.edited = true;
                        ev.changed = true;
                        ev.commit = true;
                    }
                    for v in &f.options.values {
                        if dot_item(ui, v.color.as_deref().map(color_of).unwrap_or(Color32::GRAY), &v.display(lang), cur == Some(v.id)).clicked() {
                            d.value = serde_json::json!(v.id);
                            d.edited = true;
                            ev.changed = true;
                            ev.commit = true;
                            ui.close();
                        }
                    }
                });
            });
        }
        FieldKind::Multi => {
            if f.options.values.is_empty() {
                ui.label(RichText::new(lang.t("(keine Werte definiert)", "(no values defined)")).color(style::dim(ui)));
            }
            let order: Vec<i64> = f.options.values.iter().map(|v| v.id).collect();
            let on: Vec<i64> = d.value.as_array().map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
                for v in &f.options.values {
                    let col = v.color.as_deref().map(color_of).unwrap_or(Color32::GRAY);
                    if style::pill(ui, &v.display(lang), None, col, on.contains(&v.id)).clicked() {
                        d.toggle_option(v.id, &order);
                        ev.changed = true;
                        ev.commit = true;
                    }
                }
            });
        }
        FieldKind::Labels => {
            let list: Vec<String> = d.value.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
            let accent = style::accent();
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
                for l in &list {
                    if chip_removable(ui, l, accent, lang.t("Label entfernen", "Remove label")) {
                        d.remove_label(l);
                        ev.changed = true;
                        ev.commit = true;
                    }
                }
            });
            let id = salt.with("add");
            let r = ui.add_sized([220.0, 26.0], egui::TextEdit::singleline(&mut d.text).id(id).hint_text(lang.t("Label eingeben, Enter fügt hinzu", "Type a label, Enter adds it")).margin(egui::Margin::symmetric(8, 4)));
            if r.changed() {
                d.err = None;
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && !d.text.trim().is_empty() {
                let t = d.text.clone();
                d.add_label(&t);
                ev.changed = true;
                ev.commit = true;
                ui.memory_mut(|m| m.request_focus(id));
            }
            let q = d.text.trim().to_lowercase();
            if !q.is_empty() {
                let sugg: Vec<&String> = ic.labels.iter().filter(|l| l.to_lowercase().contains(&q) && !list.iter().any(|x| x.eq_ignore_ascii_case(l))).take(6).collect();
                if !sugg.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(lang.t("Vorhanden:", "Existing:")).small().color(style::dim(ui)));
                        for s in sugg {
                            if ui.small_button(s).clicked() {
                                d.add_label(s);
                                ev.changed = true;
                                ev.commit = true;
                            }
                        }
                    });
                }
            }
        }
        FieldKind::Ticket => {
            let exclude: Vec<i64> = ic.ticket_id.into_iter().collect();
            match d.value.as_i64() {
                Some(id) => {
                    if ticket_chip(ui, ic, id, &mut ev, !f.required || ic.ticket_id.is_none()) {
                        d.value = Value::Null;
                        d.edited = true;
                        ev.changed = true;
                        ev.commit = true;
                    }
                }
                None => {
                    let mut text = std::mem::take(&mut d.text);
                    let hint = lang.t("Ticket suchen (Schlüssel oder Titel) …", "Search a ticket (key or title) …");
                    if let Some(t) = ticket_search(ui, ic, salt, &mut text, Some(ic.project_id), &f.options.target_types, &exclude, hint) {
                        d.value = serde_json::json!(t.id);
                        d.edited = true;
                        ev.changed = true;
                        ev.commit = true;
                    }
                    d.text = text;
                }
            }
        }
        FieldKind::Terminal | FieldKind::Version | FieldKind::Milestone => {
            // Der Terminal-Wert steht im Text, Version/Meilenstein als Kennung im Wert.
            let mut v = if f.kind == FieldKind::Terminal { Value::String(d.text.clone()) } else { d.value.clone() };
            let allow_clear = !f.required || ic.ticket_id.is_none();
            let mut jump = None;
            if ref_value_editor(ui, ic, salt, f.kind, &mut v, allow_clear, &mut jump) {
                if f.kind == FieldKind::Terminal {
                    d.text = v.as_str().unwrap_or("").to_string();
                } else {
                    d.value = v;
                }
                d.edited = true;
                d.err = None;
                ev.changed = true;
                ev.commit = true;
            }
            ev.open_session = jump;
        }
        FieldKind::Rich => {
            let rows = f.options.rows.unwrap_or(6).clamp(2, 40) as usize;
            let prefixes = ic.prefixes;
            let known = move |p: &str| prefixes.iter().any(|k| k == p);
            let opts = FieldOpts { rows, known_prefix: Some(&known), max_height: Some(440.0), ..Default::default() };
            let out = rich_text_field_ex(ui, salt, &mut d.text, &mut d.mode, &opts);
            if out.response.changed() {
                d.edited = true;
                ev.changed = true;
            }
            if let Some(k) = out.render.clicked_ticket {
                ev.open_key = Some(k);
            }
            if let Some(u) = out.render.clicked_url {
                open_url(&u);
            }
            if explicit_save && d.edited {
                ui.add_space(S1);
                let te_id = ui.make_persistent_id(salt).with("edit");
                let ctrl_enter = ui.memory(|m| m.has_focus(te_id)) && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter));
                ui.horizontal(|ui| {
                    if widgets::primary(ui, lang.t("Speichern", "Save")).on_hover_text("Ctrl+Enter").clicked() || ctrl_enter {
                        ev.commit = true;
                    }
                    if widgets::secondary(ui, lang.t("Verwerfen", "Discard")).clicked() {
                        ev.revert = true;
                    }
                });
            }
        }
    }
    ev
}

/// Ein Feld mit Beschriftung (Pflicht-Stern, Hilfetext) und Fehlermeldung. Kurze Felder: Beschriftung links; lange darüber.
pub(super) fn field_block(ui: &mut Ui, ic: &InCx, f: &Field, d: &mut Draft, explicit_save: bool, extra_err: Option<&str>) -> Event {
    let lang = ic.lang;
    let name = f.display(lang);
    let wide = matches!(f.kind, FieldKind::Rich | FieldKind::Multi | FieldKind::Labels | FieldKind::Ticket | FieldKind::Terminal);
    let label = |ui: &mut Ui| {
        let mut t = RichText::new(&name).color(style::dim(ui));
        if f.required {
            t = t.strong();
        }
        let r = ui.add(egui::Label::new(t).truncate());
        if f.required {
            ui.label(RichText::new("*").color(widgets::DANGER).strong()).on_hover_text(lang.t("Pflichtfeld", "Required field"));
        }
        if !f.help.is_empty() {
            r.on_hover_text(&f.help);
        }
    };
    let mut ev = Event::default();
    if wide {
        ui.horizontal(label);
        ev = field_input(ui, ic, f, d, explicit_save);
    } else {
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(Vec2::new(160.0, 26.0), egui::Layout::left_to_right(egui::Align::Center), label);
            ev = field_input(ui, ic, f, d, explicit_save);
        });
    }
    if !f.help.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(if wide { 0.0 } else { 160.0 });
            widgets::hint(ui, &f.help);
        });
    }
    let msg = d.err.as_deref().or(extra_err);
    if let Some(m) = msg {
        ui.horizontal_wrapped(|ui| {
            ui.add_space(if wide { 0.0 } else { 160.0 });
            widgets::error(ui, m);
        });
    }
    ui.add_space(S2);
    ev
}
