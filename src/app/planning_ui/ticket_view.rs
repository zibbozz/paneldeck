//! Die Ticket-Ansicht: eigene Seite mit Zurück-Pfeil oder Seitenpanel rechts neben der Liste (umschaltbar, die Wahl wird
//! gemerkt). Kopf mit Typ, Schlüssel und Titel (direkt editierbar), Status mit den erlaubten Übergängen, Stern und Markierung,
//! alle Felder in Abschnitten, Eltern und Kinder mit Fortschritt, Verknüpfungen, Zeiterfassung, Kommentare und Verlauf.
//!
//! Das Zeichnen ([`draw`]) verändert nichts an den Daten: es sammelt [`Act`]-Aufträge, die danach von
//! [`PlanUi::ticket_act`] über die Datenschicht ausgeführt werden (immer mit der Quelle „Oberfläche“).

use super::helpers::color_of;
use super::ref_display::terminal_link_view;
use super::ticket_data::{link_offers, progress_text, ChildRow, Detail, LinkOffer};
use super::ticket_draft::group_sections;
use super::ticket_inputs::{dot_item, field_block, open_url, ticket_search, InCx};
use super::ticket_state::{ViewMode, ViewState};
use super::widgets;
use super::{Cx, PlanUi};
use crate::calc::duration::{format_duration, DEFAULT_WORK_HOURS_PER_DAY};
use crate::markdown::{parse, render, rich_text_field_ex, FieldMode, FieldOpts, RenderOpts};
use crate::planning::comments::{add_comment, edit_comment, Comment};
use crate::planning::history::Source;
use crate::planning::link_types::{LinkType, TargetKind};
use crate::planning::links::{add_link, remove_link, LinkTarget, LinkView, Target};
use crate::planning::ticket_error::{FieldLabel, TicketError};
use crate::planning::tickets::{get_ticket, set_flagged, set_parent, set_starred, update_ticket, Ticket, TicketUpdate};
use crate::planning::transition::transition_ticket;
use crate::state::Lang;
use crate::style::{self, S1, S2};
use eframe::egui::{self, Align, Color32, Id, Key, Layout, Modifiers, Pos2, Rect, Response, RichText, Sense, Stroke, Ui, Vec2};
use rusqlite::Connection;

#[allow(non_snake_case)]
fn GOLD() -> Color32 {
    crate::style::ui().ticket_star
}
#[allow(non_snake_case)]
fn FLAG_RED() -> Color32 {
    crate::style::ui().ticket_flag
}

/// Was die Oberfläche im Ticket ausgelöst hat; ausgeführt wird erst nach dem Zeichnen.
#[derive(Debug)]
pub(super) enum Act {
    /// Privat-Schalter (#386).
    Private(bool),
    /// Aus dem Papierkorb holen (#386).
    Untrash,
    Back,
    Close,
    /// Schnellmenü („.“) für dieses Ticket öffnen.
    Quick,
    /// Knopf einer Schnellaktion des Typs: die Aktion (Kennung) für dieses Ticket starten.
    QuickAction(i64),
    SetMode(ViewMode),
    /// Ticket über den Schlüssel öffnen (`PAN-12`).
    OpenKey(String),
    OpenId(i64),
    /// Zur Terminal-Sitzung springen (Sitzungskennung).
    OpenSession(u64),
    Title(String),
    Star(bool),
    Flag(bool),
    /// Archivieren bzw. aus dem Archiv holen (#308).
    Archive(bool),
    Transition { to: i64, needs: Vec<FieldLabel> },
    CommitField(i64),
    RevertField(i64),
    SetParent(Option<i64>),
    CreateChild,
    Clone,
    ChangeType,
    Delete,
    AddLink { offer: LinkOffer, target: Target },
    RemoveLink(i64),
    AddComment(String),
    SaveComment { id: i64, text: String },
    DeleteComment(i64),
    LogTime,
    EditTime(i64),
    DeleteTime(i64),
}

/// Alles, was das Zeichnen von außen braucht.
pub(super) struct VCx<'a> {
    pub conn: &'a Connection,
    pub lang: Lang,
    pub link_types: &'a [LinkType],
    pub prefixes: &'a [String],
    pub labels: &'a super::ticket_data::LabelCounts,
    pub mode: ViewMode,
    /// Im Seitenpanel (schmaler, mit Schließen-Kreuz) statt auf der eigenen Seite.
    pub panel: bool,
    /// Es gibt ein vorheriges Ticket (Navigation über Verweise).
    pub has_back: bool,
    /// Name des Reiters, zu dem „Zurück“ führt (Board, Backlog …), wenn das Ticket von dort geöffnet wurde.
    pub back_to: Option<&'static str>,
}

// ------------------------------------------------------------------ Werkzeug-Icons

#[derive(Clone, Copy)]
enum Tool {
    Back,
    Page,
    Panel,
    Star,
    Flag,
}

fn tool_btn(ui: &mut Ui, tool: Tool, tip: &str, active: bool, tint: Option<Color32>) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
    let hover = resp.hovered();
    let v = ui.visuals();
    if active {
        ui.painter().rect_filled(rect, 6.0, tint.unwrap_or(style::accent()).gamma_multiply(0.25));
    } else if hover {
        ui.painter().rect_filled(rect, 6.0, v.widgets.hovered.bg_fill);
    }
    let col = match tint {
        Some(t) if active => t,
        _ if hover || active => v.strong_text_color(),
        _ => v.text_color().gamma_multiply(0.75),
    };
    let c = rect.center();
    let st = Stroke::new(1.5_f32, col);
    let p = ui.painter();
    match tool {
        Tool::Back => {
            p.line_segment([c + Vec2::new(6.0, 0.0), c + Vec2::new(-6.0, 0.0)], st);
            p.line_segment([c + Vec2::new(-6.0, 0.0), c + Vec2::new(-1.5, -4.5)], st);
            p.line_segment([c + Vec2::new(-6.0, 0.0), c + Vec2::new(-1.5, 4.5)], st);
        }
        Tool::Page => {
            p.rect_stroke(Rect::from_center_size(c, Vec2::new(14.0, 11.0)), 2.0, st, egui::StrokeKind::Inside);
            p.line_segment([c + Vec2::new(-4.0, -2.0), c + Vec2::new(4.0, -2.0)], Stroke::new(1.0_f32, col));
            p.line_segment([c + Vec2::new(-4.0, 1.5), c + Vec2::new(2.0, 1.5)], Stroke::new(1.0_f32, col));
        }
        Tool::Panel => {
            let r = Rect::from_center_size(c, Vec2::new(14.0, 11.0));
            p.rect_stroke(r, 2.0, st, egui::StrokeKind::Inside);
            p.line_segment([Pos2::new(r.right() - 5.0, r.top()), Pos2::new(r.right() - 5.0, r.bottom())], st);
            p.rect_filled(Rect::from_min_max(Pos2::new(r.right() - 5.0, r.top() + 1.0), Pos2::new(r.right() - 1.0, r.bottom() - 1.0)), 1.0, col.gamma_multiply(0.5));
        }
        Tool::Star => super::ticket_tab::paint_mark(p, "star", c, active, col),
        Tool::Flag => super::ticket_tab::paint_mark(p, "flag", c, active, col),
    }
    if hover {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.on_hover_text(tip)
}

/// Titel direkt editierbar: speichert beim Verlassen des Felds oder mit Enter, Esc verwirft.
fn title_edit(ui: &mut Ui, id: Id, shown: &str, width: f32, hint: &str) -> Option<String> {
    let buf_id = id.with("buf");
    let focused = ui.memory(|m| m.has_focus(id));
    let mut buf: String = if focused { ui.data_mut(|d| d.get_temp::<String>(buf_id)).unwrap_or_else(|| shown.to_string()) } else { shown.to_string() };
    let te = egui::TextEdit::singleline(&mut buf).id(id).font(egui::FontId::proportional(21.0)).desired_width(width).hint_text(hint).margin(egui::Margin::symmetric(8, 5));
    let r = ui.add_sized([width, 36.0], te);
    if r.changed() {
        ui.data_mut(|d| d.insert_temp(buf_id, buf.clone()));
    }
    if r.lost_focus() {
        let esc = ui.input(|i| i.key_pressed(Key::Escape));
        let stored: Option<String> = ui.data_mut(|d| d.remove_temp(buf_id));
        if !esc {
            let v = stored.unwrap_or(buf);
            if v != shown {
                return Some(v);
            }
        }
    }
    None
}

fn short_time(s: &str) -> String {
    s.replace('T', " ").chars().take(16).collect()
}

/// Zeile für ein Ticket (Typ-Marke, Schlüssel, Titel, Status); `true`, wenn angeklickt.
fn ticket_row(ui: &mut Ui, d: &Detail, t: &Ticket, done: bool, lang: Lang) -> bool {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 28.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 5.0, ui.visuals().widgets.hovered.bg_fill.gamma_multiply(0.8));
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let ty = d.sum.type_by_id(t.type_id);
    if let Some(ty) = ty {
        widgets::paint_type_badge(ui.painter(), Rect::from_min_size(Pos2::new(rect.left() + 4.0, rect.center().y - 9.0), Vec2::splat(18.0)), ty.ticket_type.icon.as_deref(), color_of(&ty.ticket_type.color));
    }
    let dim = style::dim(ui);
    let key = ui.painter().layout_no_wrap(t.key(), egui::FontId::monospace(12.5), dim);
    let kw = key.size().x;
    ui.painter().galley(Pos2::new(rect.left() + 28.0, rect.center().y - key.size().y / 2.0), key, dim);
    let status = t.status_id.and_then(|s| d.sum.status(s));
    let status_w = status.map(|s| ui.painter().layout_no_wrap(s.display(lang), egui::FontId::proportional(11.5), Color32::WHITE).size().x + 30.0).unwrap_or(0.0);
    // Archivierte Kinder bleiben sichtbar, aber gekennzeichnet und gedämpft.
    let col = if done || t.archived { dim } else { ui.visuals().text_color() };
    let title = if t.archived { format!("{} ({})", t.title, lang.t("archiviert", "archived")) } else { t.title.clone() };
    let mut job = egui::text::LayoutJob::simple_singleline(title, egui::TextStyle::Body.resolve(ui.style()), col);
    if done {
        job.sections[0].format.strikethrough = Stroke::new(1.0_f32, col);
    }
    job.wrap.max_width = (w - 36.0 - kw - status_w - 8.0).max(30.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    let g = ui.painter().layout_job(job);
    ui.painter().galley(Pos2::new(rect.left() + 36.0 + kw, rect.center().y - g.size().y / 2.0), g, col);
    if let Some(s) = status {
        let c = color_of(&s.color);
        let p = ui.painter();
        let g = p.layout_no_wrap(s.display(lang), egui::FontId::proportional(11.5), c);
        let pr = Rect::from_min_size(Pos2::new(rect.right() - status_w - 4.0, rect.center().y - 10.0), Vec2::new(g.size().x + 22.0, 20.0));
        p.rect_filled(pr, 10.0, c.gamma_multiply(0.18));
        p.circle_filled(Pos2::new(pr.left() + 10.0, pr.center().y), 3.2, c);
        p.galley(Pos2::new(pr.left() + 18.0, pr.center().y - g.size().y / 2.0), g, c);
    }
    resp.on_hover_text(&t.title).clicked()
}

/// Abschnitt mit einklappbarer Kopfzeile.
fn section<R>(ui: &mut Ui, id: impl std::hash::Hash, title: &str, open: bool, add: impl FnOnce(&mut Ui) -> R) {
    egui::CollapsingHeader::new(RichText::new(title).strong().size(15.0)).id_salt(id).default_open(open).show(ui, |ui| {
        ui.add_space(S1);
        add(ui);
        ui.add_space(S2);
    });
}

// ------------------------------------------------------------------ Zeichnen

/// Zeichnet die Ticket-Ansicht und liefert die ausgelösten Aufträge.
pub(super) fn draw(ui: &mut Ui, vc: &VCx, d: &Detail, view: &mut ViewState) -> Vec<Act> {
    let lang = vc.lang;
    let mut acts: Vec<Act> = Vec::new();
    let t = &d.ticket;
    let ty = &d.ty.ticket_type;
    let tcol = color_of(&ty.color);

    // ---- Kopf
    ui.horizontal(|ui| {
        if vc.panel {
            if style::icon_btn(ui, style::Icon::Close, lang.t("Schließen (Esc)", "Close (Esc)"), false).clicked() {
                acts.push(Act::Close);
            }
        } else if tool_btn(ui, Tool::Back, &back_tip(lang, vc.has_back, vc.back_to), false, None).clicked() {
            acts.push(Act::Back);
        }
        if vc.panel && vc.has_back && tool_btn(ui, Tool::Back, lang.t("Zurück zum vorherigen Ticket", "Back to the previous ticket"), false, None).clicked() {
            acts.push(Act::Back);
        }
        let (r, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
        widgets::paint_type_badge(ui.painter(), r, ty.icon.as_deref(), tcol);
        ui.label(RichText::new(ty.display(lang)).small().color(tcol));
        if let Some(p) = &d.parent {
            if ui.add(egui::Label::new(RichText::new(p.key()).monospace().small().color(style::dim(ui))).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(format!("{} {}", lang.t("Eltern-Ticket öffnen:", "Open parent:"), p.title)).clicked() {
                acts.push(Act::OpenId(p.id));
            }
            ui.label(RichText::new("›").color(style::dim(ui)));
        }
        ui.label(RichText::new(t.key()).monospace().strong());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.menu_button(RichText::new("⋯").strong(), |ui| {
                if ui.button(format!("{} (.)", lang.t("Schnellmenü …", "Quick menu …"))).clicked() {
                    acts.push(Act::Quick);
                    ui.close();
                }
                if ui.button(lang.t("Klonen …", "Clone …")).clicked() {
                    acts.push(Act::Clone);
                    ui.close();
                }
                if ui.button(lang.t("Typ ändern / verschieben …", "Change type / move …")).clicked() {
                    acts.push(Act::ChangeType);
                    ui.close();
                }
                if ui.button(if t.archived { lang.t("Aus dem Archiv holen", "Restore from archive") } else { lang.t("Archivieren", "Archive") }).clicked() {
                    acts.push(Act::Archive(!t.archived));
                    ui.close();
                }
                // Privat (#386): nie für Agenten sichtbar.
                if ui.button(if t.private { lang.t("Nicht mehr privat", "Make not private") } else { lang.t("Privat (nie für Agenten)", "Private (never for agents)") }).clicked() {
                    acts.push(Act::Private(!t.private));
                    ui.close();
                }
                ui.separator();
                if ui.button(RichText::new(lang.t("Löschen …", "Delete …")).color(crate::style::ui().error)).clicked() {
                    acts.push(Act::Delete);
                    ui.close();
                }
            })
            .response
            .on_hover_text(lang.t("Weitere Aktionen", "More actions"));
            if tool_btn(ui, Tool::Flag, if t.flagged { lang.t("Markierung entfernen", "Remove the flag") } else { lang.t("Markieren (Flag)", "Flag") }, t.flagged, Some(FLAG_RED())).clicked() {
                acts.push(Act::Flag(!t.flagged));
            }
            if tool_btn(ui, Tool::Star, if t.starred { lang.t("Aus den Favoriten entfernen", "Remove from favorites") } else { lang.t("Zu den Favoriten", "Add to favorites") }, t.starred, Some(GOLD())).clicked() {
                acts.push(Act::Star(!t.starred));
            }
            ui.add_space(S2);
            if tool_btn(ui, Tool::Panel, lang.t("Als Seitenpanel neben der Liste", "As a side panel next to the list"), vc.mode == ViewMode::Panel, None).clicked() {
                acts.push(Act::SetMode(ViewMode::Panel));
            }
            if tool_btn(ui, Tool::Page, lang.t("Als eigene Seite", "As a separate page"), vc.mode == ViewMode::Page, None).clicked() {
                acts.push(Act::SetMode(ViewMode::Page));
            }
        });
    });
    ui.add_space(S1);
    // ---- Papierkorb, privat, Herkunft (#386)
    if t.trashed {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(lang.t("Im Papierkorb (von einem Agenten gelöscht).", "In the trash (deleted by an agent).")).small().color(crate::style::ui().error));
            if ui.small_button(lang.t("Wiederherstellen", "Restore")).clicked() {
                acts.push(Act::Untrash);
            }
        });
        ui.add_space(S1);
    }
    if t.private || t.origin.is_system() {
        ui.horizontal_wrapped(|ui| {
            if t.private {
                ui.label(RichText::new(lang.t("Privat – für Agenten unsichtbar", "Private – invisible to agents")).small().color(Color32::from_rgb(0x9a, 0x8c, 0xe0)));
            }
            if t.origin.is_system() {
                let who = match &t.origin_detail {
                    Some(d) => format!("{} ({d})", t.origin.label(lang)),
                    None => t.origin.label(lang).to_string(),
                };
                ui.label(RichText::new(format!("{}: {who}", lang.t("Systemnotiz, angelegt von", "System note, created by"))).small().color(crate::style::dim(ui)));
            }
        });
        ui.add_space(S1);
    }
    // ---- archiviert (#308): Hinweis und „Wiederherstellen“
    if t.archived && !t.trashed {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(lang.t("Archiviert – aus Listen, Boards und Suche ausgeblendet (Suche: archived = true).", "Archived – hidden from lists, boards and search (search: archived = true).")).small().color(Color32::from_rgb(0xe5, 0xa0, 0x4b)));
            if ui.small_button(lang.t("Wiederherstellen", "Restore")).clicked() {
                acts.push(Act::Archive(false));
            }
        });
        ui.add_space(S1);
    }
    // ---- Titel
    let title_w = ui.available_width();
    if let Some(new) = title_edit(ui, Id::new(("ticket_title", t.id)), &t.title, title_w, lang.t("Titel", "Title")) {
        acts.push(Act::Title(new));
    }
    ui.add_space(S1);
    // ---- Status
    ui.horizontal_wrapped(|ui| {
        status_control(ui, vc, d, &mut acts);
        ui.add_space(S2);
        ui.label(RichText::new(format!("{} {}", lang.t("Angelegt", "Created"), short_time(&t.created_at))).small().color(style::dim(ui)));
        ui.label(RichText::new("·").small().color(style::dim(ui)));
        ui.label(RichText::new(format!("{} {}", lang.t("Geändert", "Updated"), short_time(&t.updated_at))).small().color(style::dim(ui)));
        // Fälligkeit mit Markierung (nach den Schwellen des Projekts; erledigt = ohne Markierung)
        if let Some(due) = d.ty.fields.iter().find(|f| f.is_due()).and_then(|f| d.stored.get(&f.id)).and_then(|v| v.as_str()) {
            ui.label(RichText::new("·").small().color(style::dim(ui)));
            let st = if d.done { None } else { crate::due::evaluate(&d.due_thresholds, due, chrono::Local::now().naive_local()) };
            ui.label(RichText::new(lang.t("Fällig", "Due")).small().color(style::dim(ui)));
            super::due_ui::due_line(ui, due, st.as_ref(), lang);
        }
    });
    quick_bar(ui, vc, d, &mut acts);
    ui.add_space(S2);
    ui.separator();
    ui.add_space(S1);

    // ---- Felder
    {
        let ic = InCx { conn: vc.conn, lang, project_id: t.project_id, ticket_id: Some(t.id), prefixes: vc.prefixes, labels: vc.labels, id: Id::new(("tview", t.id)) };
        if d.ty.fields.is_empty() {
            widgets::hint(ui, lang.t("Dieser Typ hat keine Felder.", "This type has no fields."));
        }
        for (name, idxs) in group_sections(&d.ty.fields) {
            let body = |ui: &mut Ui, acts: &mut Vec<Act>, view: &mut ViewState| {
                for i in idxs.iter().copied() {
                    let f = &d.ty.fields[i];
                    let Some(dr) = view.drafts.get_mut(&f.id) else { continue };
                    let ev = field_block(ui, &ic, f, dr, true, None);
                    if ev.commit {
                        acts.push(Act::CommitField(f.id));
                    }
                    if ev.revert {
                        acts.push(Act::RevertField(f.id));
                    }
                    if let Some(k) = ev.open_key {
                        acts.push(Act::OpenKey(k));
                    }
                    if let Some(s) = ev.open_session {
                        // Die Anwendung holt den Sprung im nächsten Bild ab (siehe `terminal_links::request_jump`).
                        crate::app::terminal_links::request_jump(s);
                        ui.ctx().request_repaint();
                    }
                }
            };
            if name.is_empty() {
                body(ui, &mut acts, view);
            } else {
                egui::CollapsingHeader::new(RichText::new(&name).strong()).id_salt(("fsec", t.id, &name)).default_open(true).show(ui, |ui| body(ui, &mut acts, view));
            }
        }
    }
    ui.add_space(S2);

    // ---- Eltern und Kinder
    hierarchy(ui, vc, d, view, &mut acts);
    // ---- Verknüpfungen
    links_section(ui, vc, d, view, &mut acts);
    // ---- Zeit
    time_section(ui, vc, d, &mut acts);
    // ---- Kommentare
    comments_section(ui, vc, d, view, &mut acts);
    // ---- Verlauf
    section(ui, ("hist", t.id), &match lang {
        Lang::De => format!("Verlauf ({})", d.history_total),
        Lang::En => format!("History ({})", d.history_total),
    }, false, |ui| {
        if d.history.is_empty() {
            widgets::hint(ui, lang.t("Noch keine Einträge.", "No entries yet."));
        }
        for (at, text) in &d.history {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(at).small().monospace().color(style::dim(ui)));
                ui.label(text);
            });
        }
        if d.history_total > d.history.len() {
            widgets::hint(ui, &match lang {
                Lang::De => format!("… und {} ältere Einträge.", d.history_total - d.history.len()),
                Lang::En => format!("… and {} older entries.", d.history_total - d.history.len()),
            });
        }
    });
    ui.add_space(30.0);
    acts
}

/// Breitenbereich des Ticket-Panels bei `total` Punkten Fensterbreite: höchstens 70 % und so, dass die Liste daneben
/// mindestens 360 Punkte behält, nie schmaler als 320 (sonst schrumpfte die Liste in schmalen Fenstern auf nichts, #236).
pub(super) fn panel_width_range(total: f32) -> (f32, f32) {
    let max = (total - 360.0).min(total * 0.7).max(320.0);
    (320.0_f32.min(max), max)
}

// ------------------------------------------------------------------ Schnellaktionen

/// So viele Schnellaktionen stehen höchstens als Knöpfe in der Leiste; weitere im „⋯“-Menü.
const QUICK_INLINE_MAX: usize = 6;

/// Leiste mit den Schnellaktionen des Tickettyps als Knöpfe (Reihenfolge wie im Typ-Editor). Ein Klick startet die Aktion
/// über das Schnellmenü (`Act::QuickAction`); ungültige oder gesperrte Aktionen sind ausgegraut und erklären sich im Hinweis.
fn quick_bar(ui: &mut Ui, vc: &VCx, d: &Detail, acts: &mut Vec<Act>) {
    if d.quick.is_empty() {
        return;
    }
    let lang = vc.lang;
    let tid = d.ticket.id;
    let (shown, rest) = if d.quick.len() > QUICK_INLINE_MAX { d.quick.split_at(QUICK_INLINE_MAX - 1) } else { (&d.quick[..], &d.quick[d.quick.len()..]) };
    ui.add_space(S1);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
        ui.label(RichText::new(lang.t("Schnellaktionen", "Quick actions")).small().color(style::dim(ui)));
        for b in shown {
            ui.push_id(("qbtn", tid, b.action_id), |ui| {
                let btn = egui::Button::new(b.label.as_str()).fill(style::accent().gamma_multiply(0.18)).min_size(Vec2::new(0.0, 24.0));
                let resp = ui.add_enabled(b.disabled.is_none(), btn);
                #[cfg(test)]
                ui.ctx().data_mut(|data| data.insert_temp(Id::new(("qbtn_rect", b.action_id)), resp.rect));
                let resp = if b.disabled.is_none() { resp.on_hover_text(&b.tip) } else { resp.on_disabled_hover_text(&b.tip) };
                if resp.clicked() {
                    acts.push(Act::QuickAction(b.action_id));
                }
            });
        }
        if !rest.is_empty() {
            ui.push_id(("qbtn_more", tid), |ui| {
                let more = match lang {
                    Lang::De => format!("⋯ {} weitere", rest.len()),
                    Lang::En => format!("⋯ {} more", rest.len()),
                };
                ui.menu_button(more, |ui| {
                    for b in rest {
                        ui.push_id(("qbtn", tid, b.action_id), |ui| {
                            let resp = ui.add_enabled(b.disabled.is_none(), egui::Button::new(b.label.as_str()));
                            let resp = if b.disabled.is_none() { resp.on_hover_text(&b.tip) } else { resp.on_disabled_hover_text(&b.tip) };
                            if resp.clicked() {
                                acts.push(Act::QuickAction(b.action_id));
                                ui.close();
                            }
                        });
                    }
                });
            });
        }
    });
}

// ------------------------------------------------------------------ Status

fn status_control(ui: &mut Ui, vc: &VCx, d: &Detail, acts: &mut Vec<Act>) {
    let lang = vc.lang;
    let t = &d.ticket;
    if !d.ty.ticket_type.workflow {
        ui.label(RichText::new(lang.t("Dieser Typ hat keinen Status.", "This type has no status.")).small().color(style::dim(ui)));
        return;
    }
    let cur = t.status_id.and_then(|s| d.sum.status(s));
    let (name, col) = match cur {
        Some(s) => (s.display(lang), color_of(&s.color)),
        None => ("–".to_string(), Color32::GRAY),
    };
    let (r, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
    ui.painter().circle_filled(r.center(), 5.0, col);
    egui::ComboBox::from_id_salt(("status_pick", t.id)).selected_text(RichText::new(name).strong()).width(170.0).show_ui(ui, |ui| {
        if d.transitions.is_empty() {
            ui.label(RichText::new(lang.t("Kein weiterer Statuswechsel erlaubt.", "No further status change is allowed.")).small().color(style::dim(ui)));
        }
        for o in &d.transitions {
            let Some(s) = d.sum.status(o.status_id) else { continue };
            let mut label = s.display(lang);
            if !o.needs_fields.is_empty() {
                label.push_str(" …");
            }
            let resp = ui.add_enabled_ui(!o.blocked, |ui| dot_item(ui, color_of(&s.color), &label, false)).inner;
            if o.blocked {
                resp.on_disabled_hover_text(lang.t("Gesperrt: Eine Bedingung des Übergangs ist nicht erfüllt.", "Blocked: a condition of the transition is not met."));
            } else if !o.needs_fields.is_empty() {
                let names = o.needs_fields.iter().map(|f| f.text(lang).to_string()).collect::<Vec<_>>().join(", ");
                let tip = match lang {
                    Lang::De => format!("Dafür müssen noch Felder ausgefüllt werden: {names}"),
                    Lang::En => format!("Fields still need to be filled in: {names}"),
                };
                if resp.on_hover_text(tip).clicked() {
                    acts.push(Act::Transition { to: o.status_id, needs: o.needs_fields.clone() });
                    ui.close();
                }
            } else if resp.clicked() {
                acts.push(Act::Transition { to: o.status_id, needs: Vec::new() });
                ui.close();
            }
        }
    });
    if d.done {
        ui.label(RichText::new(lang.t("erledigt", "done")).small().color(Color32::from_rgb(0x7f, 0xbf, 0x6a)));
    }
}

// ------------------------------------------------------------------ Eltern und Kinder

fn hierarchy(ui: &mut Ui, vc: &VCx, d: &Detail, view: &mut ViewState, acts: &mut Vec<Act>) {
    let lang = vc.lang;
    let t = &d.ticket;
    let can_have_parent = !d.ty.parents.is_empty();
    let child_types = d.sum.types.iter().any(|x| !x.ticket_type.system && x.parents.contains(&d.ty.ticket_type.id));
    if !can_have_parent && d.parent.is_none() && d.children.is_empty() && !child_types {
        return;
    }
    section(ui, ("hier", t.id), lang.t("Hierarchie", "Hierarchy"), true, |ui| {
        if can_have_parent || d.parent.is_some() {
            ui.horizontal_wrapped(|ui| {
                ui.allocate_ui_with_layout(Vec2::new(110.0, 26.0), Layout::left_to_right(Align::Center), |ui| {
                    ui.label(RichText::new(lang.t("Eltern-Ticket", "Parent")).color(style::dim(ui)));
                });
                match &d.parent {
                    Some(p) => {
                        let w = (ui.available_width() - 150.0).clamp(160.0, 420.0);
                        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 26.0), Sense::click());
                        if resp.hovered() {
                            ui.painter().rect_filled(rect, 5.0, ui.visuals().widgets.hovered.bg_fill);
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        let text = format!("{}  {}", p.key(), p.title);
                        let mut job = egui::text::LayoutJob::simple_singleline(text, egui::TextStyle::Body.resolve(ui.style()), ui.visuals().text_color());
                        job.wrap.max_width = w - 8.0;
                        job.wrap.max_rows = 1;
                        job.wrap.break_anywhere = true;
                        job.wrap.overflow_character = Some('…');
                        let g = ui.painter().layout_job(job);
                        ui.painter().galley(Pos2::new(rect.left() + 6.0, rect.center().y - g.size().y / 2.0), g, ui.visuals().text_color());
                        if resp.on_hover_text(lang.t("Eltern-Ticket öffnen", "Open the parent ticket")).clicked() {
                            acts.push(Act::OpenId(p.id));
                        }
                        if ui.small_button(lang.t("Ändern", "Change")).clicked() {
                            view.parent_open = !view.parent_open;
                        }
                        if ui.small_button(lang.t("Entfernen", "Remove")).clicked() {
                            acts.push(Act::SetParent(None));
                        }
                    }
                    None => {
                        ui.label(RichText::new(lang.t("– keins –", "– none –")).color(style::dim(ui)));
                        if can_have_parent && ui.small_button(lang.t("Wählen …", "Choose …")).clicked() {
                            view.parent_open = !view.parent_open;
                        }
                    }
                }
            });
            if view.parent_open && can_have_parent {
                let ic = InCx { conn: vc.conn, lang, project_id: t.project_id, ticket_id: Some(t.id), prefixes: vc.prefixes, labels: vc.labels, id: Id::new(("tview", t.id)) };
                let hint = lang.t("Eltern-Ticket suchen (Schlüssel oder Titel) …", "Search the parent (key or title) …");
                let mut exclude: Vec<i64> = vec![t.id];
                exclude.extend(d.children.iter().map(|c| c.ticket.id));
                ui.indent("parent_pick", |ui| {
                    if let Some(p) = ticket_search(ui, &ic, Id::new(("parent_search", t.id)), &mut view.parent_text, Some(t.project_id), &d.ty.parents, &exclude, hint) {
                        acts.push(Act::SetParent(Some(p.id)));
                        view.parent_open = false;
                        view.parent_text.clear();
                    }
                });
            }
            ui.add_space(S1);
        }
        if !d.children.is_empty() || child_types {
            ui.horizontal(|ui| {
                ui.label(RichText::new(match lang {
                    Lang::De => format!("Kind-Tickets ({})", d.children.len()),
                    Lang::En => format!("Child tickets ({})", d.children.len()),
                }).strong());
                if child_types && ui.small_button(format!("+ {}", lang.t("Kind anlegen", "Add child"))).clicked() {
                    acts.push(Act::CreateChild);
                }
            });
            if let Some(p) = &d.progress {
                let frac = p.fraction as f32;
                widgets::labeled_bar(ui, frac, &progress_text(p, lang), 420.0);
                ui.add_space(S1);
            }
            for ChildRow { ticket, done } in &d.children {
                if ticket_row(ui, d, ticket, *done, lang) {
                    acts.push(Act::OpenId(ticket.id));
                }
            }
        }
    });
}

// ------------------------------------------------------------------ Verknüpfungen

fn link_target_text(v: &LinkView) -> String {
    match &v.target {
        LinkTarget::Ticket(r) => format!("{}  {}", r.key, r.title),
        LinkTarget::Terminal(s) => s.clone(),
        LinkTarget::Url(u) => u.clone(),
        LinkTarget::Branch { repo, name } => if repo.is_empty() { name.clone() } else { format!("{repo} · {name}") },
    }
}

fn links_section(ui: &mut Ui, vc: &VCx, d: &Detail, view: &mut ViewState, acts: &mut Vec<Act>) {
    let lang = vc.lang;
    let t = &d.ticket;
    let offers = link_offers(vc.link_types, lang);
    section(ui, ("links", t.id), &match lang {
        Lang::De => format!("Verknüpfungen ({})", d.links.len()),
        Lang::En => format!("Links ({})", d.links.len()),
    }, true, |ui| {
        if d.links.is_empty() {
            widgets::hint(ui, lang.t("Keine Verknüpfungen.", "No links."));
        }
        for l in &d.links {
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(Vec2::new(130.0, 24.0), Layout::left_to_right(Align::Center), |ui| {
                    ui.add(egui::Label::new(RichText::new(&l.name).small().color(style::dim(ui))).truncate());
                });
                let text = link_target_text(l);
                let w = (ui.available_width() - 34.0).max(80.0);
                match &l.target {
                    LinkTarget::Ticket(r) => {
                        let resp = ui.add_sized([w, 24.0], egui::Button::new(RichText::new(&text)).frame(false).truncate());
                        if resp.on_hover_text(lang.t("Ticket öffnen", "Open ticket")).clicked() {
                            acts.push(Act::OpenId(r.id));
                        }
                    }
                    LinkTarget::Url(u) => {
                        let resp = ui.add_sized([w, 24.0], egui::Button::new(RichText::new(&text).underline()).frame(false).truncate());
                        if resp.on_hover_text(lang.t("Im Browser öffnen", "Open in the browser")).clicked() {
                            open_url(u);
                        }
                    }
                    LinkTarget::Branch { .. } => {
                        ui.add_sized([w, 24.0], egui::Label::new(RichText::new(format!("⎇ {text}")).monospace()).truncate());
                    }
                    LinkTarget::Terminal(target) => {
                        // laufende Sitzung: Klick springt hin; beendet oder fremd: grau, nur Anzeige
                        let (name, running, tip) = terminal_link_view(target, lang);
                        match running {
                            Some(session) => {
                                let resp = ui.add_sized([w, 24.0], egui::Button::new(RichText::new(format!("⌨ {name}")).monospace().color(style::accent())).frame(false).truncate());
                                if resp.on_hover_text(tip).clicked() {
                                    acts.push(Act::OpenSession(session));
                                    // Der Sprung wird erst im nächsten Bild abgeholt: ohne neues Bild bliebe er bis zur nächsten Eingabe liegen.
                                    ui.ctx().request_repaint();
                                }
                            }
                            None => {
                                ui.add_sized([w, 24.0], egui::Label::new(RichText::new(format!("⌨ {name}")).monospace().color(style::dim(ui))).truncate()).on_hover_text(tip);
                            }
                        }
                    }
                }
                if style::icon_btn_small(ui, style::Icon::Close, lang.t("Verknüpfung entfernen", "Remove the link")).clicked() {
                    acts.push(Act::RemoveLink(l.id));
                }
            });
        }
        ui.add_space(S1);
        if !view.link.open {
            if ui.small_button(format!("+ {}", lang.t("Verknüpfung hinzufügen", "Add link"))).clicked() {
                view.link.open = true;
            }
            return;
        }
        // Formular
        let n = offers.len();
        if n == 0 {
            widgets::hint(ui, lang.t("Es gibt keinen Verknüpfungstyp, den man von Hand anlegen kann.", "There is no link type that can be added by hand."));
            return;
        }
        view.link.choice = view.link.choice.min(n - 1);
        let offer = offers[view.link.choice].clone();
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt(("link_offer", t.id)).selected_text(&offer.label).width(180.0).show_ui(ui, |ui| {
                    for (i, o) in offers.iter().enumerate() {
                        if ui.selectable_label(i == view.link.choice, &o.label).clicked() {
                            view.link.choice = i;
                            view.link.target_ticket = None;
                        }
                    }
                });
                ui.vertical(|ui| match offer.kind {
                    TargetKind::Ticket => {
                        let ic = InCx { conn: vc.conn, lang, project_id: t.project_id, ticket_id: Some(t.id), prefixes: vc.prefixes, labels: vc.labels, id: Id::new(("tview", t.id)) };
                        if let Some(id) = view.link.target_ticket {
                            ui.horizontal(|ui| {
                                if let Ok(x) = get_ticket(vc.conn, id) {
                                    ui.label(RichText::new(x.key()).monospace().strong());
                                    ui.label(&x.title);
                                }
                                if ui.small_button("✕").clicked() {
                                    view.link.target_ticket = None;
                                }
                            });
                        } else {
                            let hint = lang.t("Ticket suchen (Schlüssel oder Titel) …", "Search a ticket (key or title) …");
                            if let Some(x) = ticket_search(ui, &ic, Id::new(("link_search", t.id)), &mut view.link.text, None, &[], &[t.id], hint) {
                                view.link.target_ticket = Some(x.id);
                            }
                        }
                    }
                    TargetKind::Url => {
                        ui.add(egui::TextEdit::singleline(&mut view.link.text).desired_width(320.0).hint_text("https://…").margin(egui::Margin::symmetric(8, 4)));
                    }
                    TargetKind::Branch => {
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut view.link.repo).desired_width(150.0).hint_text(lang.t("Repository (optional)", "Repository (optional)")).margin(egui::Margin::symmetric(8, 4)));
                            ui.add(egui::TextEdit::singleline(&mut view.link.name).desired_width(170.0).hint_text(lang.t("Branch-Name", "Branch name")).margin(egui::Margin::symmetric(8, 4)));
                        });
                    }
                    TargetKind::Terminal => {}
                });
            });
            ui.add_space(S1);
            ui.horizontal(|ui| {
                let target = match offer.kind {
                    TargetKind::Ticket => view.link.target_ticket.map(Target::Ticket),
                    TargetKind::Url => Some(Target::Url(view.link.text.trim().to_string())).filter(|_| !view.link.text.trim().is_empty()),
                    TargetKind::Branch => Some(Target::Branch { repo: view.link.repo.trim().to_string(), name: view.link.name.trim().to_string() }).filter(|_| !view.link.name.trim().is_empty()),
                    TargetKind::Terminal => None,
                };
                if widgets::primary_enabled(ui, target.is_some(), lang.t("Hinzufügen", "Add")).clicked() {
                    if let Some(tg) = target {
                        acts.push(Act::AddLink { offer: offer.clone(), target: tg });
                    }
                }
                if widgets::secondary(ui, lang.t("Abbrechen", "Cancel")).clicked() {
                    view.link = Default::default();
                }
            });
        });
    });
}

// ------------------------------------------------------------------ Zeit

fn time_section(ui: &mut Ui, vc: &VCx, d: &Detail, acts: &mut Vec<Act>) {
    let lang = vc.lang;
    let t = &d.ticket;
    let total = format_duration(d.time_total, DEFAULT_WORK_HOURS_PER_DAY, lang);
    section(ui, ("time", t.id), &match lang {
        Lang::De => format!("Zeiterfassung ({total})"),
        Lang::En => format!("Time tracking ({total})"),
    }, true, |ui| {
        if ui.button(format!("⏱ {}", lang.t("Zeit erfassen", "Log time"))).clicked() {
            acts.push(Act::LogTime);
        }
        ui.add_space(S1);
        if d.time.is_empty() {
            widgets::hint(ui, lang.t("Noch keine Zeit erfasst.", "No time logged yet."));
        }
        for e in &d.time {
            ui.horizontal(|ui| {
                ui.label(RichText::new(short_time(&e.start)).monospace().small().color(style::dim(ui)));
                ui.label(RichText::new(format_duration(e.minutes, DEFAULT_WORK_HOURS_PER_DAY, lang)).strong());
                if let Some(k) = e.work_kind_id.and_then(|k| d.sum.work_kinds.iter().find(|w| w.id == k)) {
                    ui.label(RichText::new(k.display(lang)).small().color(style::accent()));
                }
                let first = e.comment.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
                if !first.is_empty() {
                    ui.add(egui::Label::new(RichText::new(first).color(style::dim(ui))).truncate());
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if style::icon_btn_small(ui, style::Icon::Trash, lang.t("Eintrag löschen", "Delete the entry")).clicked() {
                        acts.push(Act::DeleteTime(e.id));
                    }
                    if ui.small_button(lang.t("Bearbeiten", "Edit")).clicked() {
                        acts.push(Act::EditTime(e.id));
                    }
                });
            });
        }
    });
}

// ------------------------------------------------------------------ Kommentare

fn comments_section(ui: &mut Ui, vc: &VCx, d: &Detail, view: &mut ViewState, acts: &mut Vec<Act>) {
    let lang = vc.lang;
    let t = &d.ticket;
    let prefixes = vc.prefixes;
    let known = move |p: &str| prefixes.iter().any(|k| k == p);
    section(ui, ("comments", t.id), &match lang {
        Lang::De => format!("Kommentare ({})", d.comments.len()),
        Lang::En => format!("Comments ({})", d.comments.len()),
    }, true, |ui| {
        if d.comments.is_empty() {
            widgets::hint(ui, lang.t("Noch keine Kommentare.", "No comments yet."));
        }
        for c in &d.comments {
            comment_card(ui, vc, c, view, acts, &known);
            ui.add_space(S1);
        }
        ui.add_space(S1);
        ui.label(RichText::new(lang.t("Neuer Kommentar", "New comment")).small().color(style::dim(ui)));
        let opts = FieldOpts { rows: 4, known_prefix: Some(&known), max_height: Some(320.0), hint: Some(lang.t("Kommentar schreiben (Markdown) …", "Write a comment (Markdown) …")), ..Default::default() };
        let out = rich_text_field_ex(ui, ("cnew", t.id), &mut view.comment, &mut view.comment_mode, &opts);
        if let Some(k) = out.render.clicked_ticket {
            acts.push(Act::OpenKey(k));
        }
        if let Some(u) = out.render.clicked_url {
            open_url(&u);
        }
        let te_id = ui.make_persistent_id(("cnew", t.id)).with("edit");
        let ctrl_enter = ui.memory(|m| m.has_focus(te_id)) && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter));
        ui.add_space(S1);
        ui.horizontal(|ui| {
            let ok = !view.comment.trim().is_empty();
            if widgets::primary_enabled(ui, ok, lang.t("Kommentar hinzufügen", "Add comment")).on_hover_text("Ctrl+Enter").clicked() || (ctrl_enter && ok) {
                acts.push(Act::AddComment(view.comment.clone()));
            }
        });
    });
}

fn comment_card(ui: &mut Ui, vc: &VCx, c: &Comment, view: &mut ViewState, acts: &mut Vec<Act>, known: &dyn Fn(&str) -> bool) {
    let lang = vc.lang;
    egui::Frame::NONE
        .fill(ui.visuals().faint_bg_color)
        .stroke(Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(short_time(&c.created_at)).small().color(style::dim(ui)));
                if c.updated_at != c.created_at {
                    ui.label(RichText::new(lang.t("(bearbeitet)", "(edited)")).small().color(style::dim(ui)));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if view.comment_edit.as_ref().is_none_or(|(id, _, _)| *id != c.id) {
                        if style::icon_btn_small(ui, style::Icon::Trash, lang.t("Kommentar löschen", "Delete the comment")).clicked() {
                            acts.push(Act::DeleteComment(c.id));
                        }
                        if ui.small_button(lang.t("Bearbeiten", "Edit")).clicked() {
                            view.comment_edit = Some((c.id, c.body.clone(), FieldMode::Text));
                        }
                    }
                });
            });
            ui.add_space(S1);
            let editing = view.comment_edit.as_ref().is_some_and(|(id, _, _)| *id == c.id);
            if editing {
                let mut save: Option<String> = None;
                let mut cancel = false;
                if let Some((_, text, mode)) = view.comment_edit.as_mut() {
                    let opts = FieldOpts { rows: 4, known_prefix: Some(known), max_height: Some(320.0), ..Default::default() };
                    let out = rich_text_field_ex(ui, ("cedit", c.id), text, mode, &opts);
                    if let Some(k) = out.render.clicked_ticket {
                        acts.push(Act::OpenKey(k));
                    }
                    let te_id = ui.make_persistent_id(("cedit", c.id)).with("edit");
                    let ctrl_enter = ui.memory(|m| m.has_focus(te_id)) && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter));
                    ui.add_space(S1);
                    ui.horizontal(|ui| {
                        let ok = !text.trim().is_empty();
                        if widgets::primary_enabled(ui, ok, lang.t("Speichern", "Save")).on_hover_text("Ctrl+Enter").clicked() || (ctrl_enter && ok) {
                            save = Some(text.clone());
                        }
                        if widgets::secondary(ui, lang.t("Abbrechen", "Cancel")).clicked() {
                            cancel = true;
                        }
                    });
                }
                if let Some(text) = save {
                    acts.push(Act::SaveComment { id: c.id, text });
                }
                if cancel {
                    view.comment_edit = None;
                }
            } else {
                let doc = parse(&c.body, known);
                let out = render(ui, &doc, &RenderOpts::default());
                if let Some(k) = out.clicked_ticket {
                    acts.push(Act::OpenKey(k));
                }
                if let Some(u) = out.clicked_url {
                    open_url(&u);
                }
            }
        });
}

// ------------------------------------------------------------------ Ausführen der Aufträge

impl PlanUi {
    /// Eigene Seite des geöffneten Tickets (Modus „Seite“): füllt den Platz unter den Reitern.
    pub(super) fn ticket_page(&mut self, ui: &mut Ui, cx: &Cx) {
        self.ticket_content(ui, cx, false);
    }

    /// Seitenpanel rechts neben der Liste (Modus „Panel“).
    pub(super) fn ticket_panel(&mut self, ctx: &egui::Context, cx: &Cx, bg: Color32) {
        let (min_w, max_w) = panel_width_range(ctx.available_rect().width());
        // Ohne Rahmenlinie: oben läge sie direkt unter der Linie des Kopfs (doppelte, hellere Kante über dem Ticket, #236);
        // links trennt die Trennlinie des Panels. Der Inhalt kann das Panel nicht verbreitern (`fixed_body`).
        egui::SidePanel::right("ticket_side_panel")
            .resizable(true)
            .default_width(540.0_f32.clamp(min_w, max_w))
            .width_range(min_w..=max_w)
            .frame(egui::Frame::NONE.fill(bg).inner_margin(egui::Margin { left: 14, right: 14, top: 12, bottom: 0 }))
            .show(ctx, |ui| {
                super::super::panels::fixed_body(ui, |ui| self.ticket_content(ui, cx, true));
            });
    }

    fn ticket_content(&mut self, ui: &mut Ui, cx: &Cx, panel: bool) {
        let lang = cx.lang;
        let Some(d) = self.tickets.detail.take() else {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| ui.label(RichText::new(lang.t("Wird geladen …", "Loading …")).color(style::dim(ui))));
            return;
        };
        let mut view = std::mem::take(&mut self.tickets.view);
        if view.ticket != d.ticket.id {
            view = ViewState { ticket: d.ticket.id, ..Default::default() };
            d.merge_drafts(&mut view.drafts);
        }
        let prefixes: Vec<String> = self.projects.iter().map(|p| p.key.clone()).collect();
        let labels = std::mem::take(&mut self.tickets.labels);
        let vc = VCx { conn: cx.conn, lang, link_types: &self.links, prefixes: &prefixes, labels: &labels, mode: self.tickets.mode, panel, has_back: !self.tickets.back.is_empty(), back_to: self.back_target_label(lang) };
        let mut acts: Vec<Act> = Vec::new();
        if let Some(m) = self.msg.as_ref().filter(|m| !m.ok) {
            let text = m.text.clone();
            ui.horizontal_wrapped(|ui| widgets::error(ui, &text));
            ui.add_space(S1);
        }
        let max_w = if panel { f32::INFINITY } else { 980.0 };
        egui::ScrollArea::vertical().id_salt(("ticket_scroll", panel)).auto_shrink([false, false]).show(ui, |ui| {
            let w = ui.available_width().min(max_w);
            let x0 = ui.max_rect().left() + ((ui.available_width() - w) / 2.0).max(0.0);
            let rect = Rect::from_min_size(Pos2::new(x0, ui.cursor().top()), Vec2::new(w, ui.available_height().max(10.0)));
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                ui.set_width(w - 12.0);
                acts = draw(ui, &vc, &d, &mut view);
            });
        });
        self.tickets.labels = labels;
        self.tickets.detail = Some(d);
        // der Puffer kann sich während der Aufträge ändern (z. B. Kommentarfeld leeren): zuerst zurückgeben
        self.tickets.view = view;
        for a in acts {
            self.ticket_act(cx, a);
        }
    }

    pub(super) fn ticket_act(&mut self, cx: &Cx, act: Act) {
        let lang = cx.lang;
        let Some(tid) = self.tickets.open else { return };
        let src = Source::ui();
        match act {
            Act::Back => self.ticket_back(),
            Act::Close => self.ticket_close(),
            Act::Quick => self.open_quick_menu(vec![tid]),
            Act::QuickAction(action) => self.open_quick_action(vec![tid], action),
            Act::SetMode(m) => {
                self.tickets.mode = m;
                cx.db.set_meta("ticket_view_mode", m.as_str());
            }
            Act::OpenKey(key) => match crate::planning::tickets::ticket_by_key(cx.conn, &key) {
                Ok(t) => self.open_ticket_anywhere(cx, t.id, t.project_id),
                Err(_) => self.msg_error(cx, match lang {
                    Lang::De => format!("Das Ticket {key} gibt es nicht."),
                    Lang::En => format!("There is no ticket {key}."),
                }),
            },
            // Die Anwendung holt den Sprung im nächsten Bild ab (siehe `terminal_links::request_jump`).
            Act::OpenSession(session) => crate::app::terminal_links::request_jump(session),
            Act::OpenId(id) => match get_ticket(cx.conn, id) {
                Ok(t) => self.open_ticket_anywhere(cx, t.id, t.project_id),
                Err(e) => self.t_fail(cx, &e),
            },
            Act::Title(s) => {
                let r = update_ticket(cx.conn, tid, &TicketUpdate { title: Some(s), values: Vec::new() }, &src);
                self.t_apply(cx, r);
                self.tickets.detail_dirty = true;
            }
            Act::Star(on) => {
                let r = set_starred(cx.conn, tid, on, &src);
                self.t_apply(cx, r);
            }
            Act::Flag(on) => {
                let r = set_flagged(cx.conn, tid, on, &src);
                self.t_apply(cx, r);
            }
            Act::Private(on) => match crate::planning::access::set_ticket_private(cx.conn, tid, on, &src) {
                Ok(_) => {
                    self.tickets.invalidate();
                    self.tickets.detail_dirty = true;
                }
                Err(e) => self.msg_error(cx, e.message(lang)),
            },
            Act::Untrash => match crate::planning::access::restore_ticket(cx.conn, tid, &src) {
                Ok(()) => {
                    self.tickets.invalidate();
                    self.tickets.detail_dirty = true;
                    self.tickets.counts_dirty = true;
                    self.dirty = true;
                }
                Err(e) => self.msg_error(cx, e.message(lang)),
            },
            Act::Archive(on) => match crate::planning::maintenance::actions::set_archived_ui(cx.conn, tid, on) {
                Ok(_) => {
                    self.tickets.invalidate();
                    self.tickets.detail_dirty = true;
                    self.tickets.counts_dirty = true;
                    self.dirty = true;
                }
                Err(e) => self.msg_error(cx, e.message(lang)),
            },
            Act::Transition { to, needs } => {
                if !needs.is_empty() {
                    self.open_transition(cx, tid, to, &needs);
                    return;
                }
                match transition_ticket(cx.conn, tid, to, &[], &src) {
                    Err(TicketError::TransitionNeedsFields(v)) => self.open_transition(cx, tid, to, &v),
                    r => {
                        self.t_apply(cx, r);
                    }
                }
            }
            Act::CommitField(fid) => self.commit_field(cx, tid, fid),
            Act::RevertField(fid) => {
                self.tickets.view.drafts.remove(&fid);
                self.tickets.detail_dirty = true;
            }
            Act::SetParent(p) => {
                let r = set_parent(cx.conn, tid, p, &src);
                self.t_apply(cx, r);
            }
            Act::CreateChild => self.open_create(cx, Some(tid)),
            Act::Clone => self.ask_clone(cx, tid),
            Act::ChangeType => self.ask_change_type(cx, tid),
            Act::Delete => self.ask_delete_ticket(cx, tid),
            Act::AddLink { offer, target } => {
                let r = match (offer.reverse, &target) {
                    (true, Target::Ticket(other)) => add_link(cx.conn, *other, offer.link_type_id, Target::Ticket(tid), &src),
                    _ => add_link(cx.conn, tid, offer.link_type_id, target, &src),
                };
                match r {
                    Ok((_, op)) => {
                        self.tickets.view.link = Default::default();
                        self.t_apply(cx, Ok(Some(op)));
                    }
                    Err(e) => self.t_fail(cx, &e),
                }
            }
            Act::RemoveLink(id) => {
                let r = remove_link(cx.conn, id, &src).map(Some);
                self.t_apply(cx, r);
            }
            Act::AddComment(text) => match add_comment(cx.conn, tid, &text, &src) {
                Ok((_, op)) => {
                    self.tickets.view.comment.clear();
                    self.tickets.view.comment_mode = FieldMode::Text;
                    self.t_apply(cx, Ok(Some(op)));
                }
                Err(e) => self.t_fail(cx, &e),
            },
            Act::SaveComment { id, text } => match edit_comment(cx.conn, id, &text, &src) {
                Ok(op) => {
                    self.tickets.view.comment_edit = None;
                    self.t_apply(cx, Ok(op));
                }
                Err(e) => self.t_fail(cx, &e),
            },
            Act::DeleteComment(id) => self.tickets.confirm = Some(super::ticket_dialogs::ConfirmDlg { kind: super::ticket_dialogs::ConfirmKind::Comment { ticket: tid, id } }),
            Act::LogTime => self.open_time(cx, tid, None),
            Act::EditTime(id) => self.open_time(cx, tid, Some(id)),
            Act::DeleteTime(id) => self.tickets.confirm = Some(super::ticket_dialogs::ConfirmDlg { kind: super::ticket_dialogs::ConfirmKind::Time { ticket: tid, id } }),
        }
    }

    /// Ein Feld speichern: Eingabe lesen, über die Datenschicht prüfen und schreiben; Fehler stehen am Feld.
    fn commit_field(&mut self, cx: &Cx, tid: i64, fid: i64) {
        let lang = cx.lang;
        let Some(f) = self.tickets.detail.as_ref().and_then(|d| d.ty.field(fid)).cloned() else { return };
        let Some(dr) = self.tickets.view.drafts.get_mut(&fid) else { return };
        match dr.to_value(&f) {
            Err(e) => dr.err = Some(e.message(lang)),
            Ok(v) => match crate::planning::tickets::set_field(cx.conn, tid, fid, v, &Source::ui()) {
                Ok(op) => {
                    dr.edited = false;
                    dr.err = None;
                    self.tickets.commit(op);
                    self.dirty = true;
                    self.msg = None;
                    self.tickets.detail_dirty = true;
                }
                Err(e) => dr.err = Some(e.message(lang)),
            },
        }
    }
}

/// Hinweistext des Zurück-Pfeils: vorheriges Ticket, Ursprungs-Reiter (Board, Backlog …) oder Liste.
fn back_tip(lang: Lang, has_back: bool, back_to: Option<&str>) -> String {
    match (has_back, back_to) {
        (true, _) => lang.t("Zurück zum vorherigen Ticket (Esc)", "Back to the previous ticket (Esc)").to_string(),
        (false, Some(tab)) => match lang {
            Lang::De => format!("Zurück zu „{tab}“ (Esc)"),
            Lang::En => format!("Back to “{tab}” (Esc)"),
        },
        (false, None) => lang.t("Zurück zur Liste (Esc)", "Back to the list (Esc)").to_string(),
    }
}
