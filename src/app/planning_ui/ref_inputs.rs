//! Eingaben für Felder, die auf eine Version, einen Meilenstein oder eine Terminal-Sitzung verweisen.
//!
//! * **Version/Meilenstein:** Auswahlliste der Einträge **des Ticket-Projekts** in der Reihenfolge der Verwaltung (Reiter
//!   „Releases“); erschienene/erreichte stehen gedämpft mit Häkchen, die Fälligkeit steht im Tooltip. Ist `use_versions` im
//!   Projekt aus, gibt es keine Liste, sondern einen Hinweis. Ein Feld nimmt genau einen Eintrag (Kennung), eine Mehrfachauswahl
//!   gibt das Datenmodell für diese Feldarten nicht her.
//! * **Terminal:** Chip mit dem Namen der Sitzung (grau und „beendet“, wenn sie nicht mehr läuft); Klick auf einen laufenden
//!   Chip springt zur Sitzung (siehe [`request_jump`](super::super::terminal_links::request_jump)), „Ändern“ öffnet die Auswahl
//!   der aktuellen Sitzungen (`pick_terminal`), das Kreuz leert das Feld.
//! * [`release_label`] liefert den Namen zu einer Kennung (mit kleinem Zwischenspeicher) für Anzeigen ohne Eingabe.
//!
//! [`ref_value_editor`] arbeitet auf einem JSON-Wert und dient der Ticket-Ansicht, den Dialogen und dem Board-Dialog
//! „Pflichtfelder ausfüllen“ gleichermaßen. Gespeichert wird nicht hier, sondern vom Aufrufer.

use super::super::terminal_links::{parse_target, pick_terminal, terminal_label, terminal_link_state, LinkState};
use super::ticket_inputs::InCx;
use super::widgets;
use crate::planning::fields::FieldKind;
use crate::planning::projects::get_project;
use crate::planning::sprints::releases::{get_release, list_releases, Release, ReleaseKind};
use crate::state::Lang;
use crate::style;
use eframe::egui::{self, Id, Pos2, Response, RichText, Sense, Stroke, Ui, Vec2};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

// ------------------------------------------------------------------ Logik (ohne egui)

/// Welche Art von Eintrag ein Feld auswählt (`None` = kein Version-/Meilenstein-Feld).
pub(super) fn release_kind(kind: FieldKind) -> Option<ReleaseKind> {
    match kind {
        FieldKind::Version => Some(ReleaseKind::Version),
        FieldKind::Milestone => Some(ReleaseKind::Milestone),
        _ => None,
    }
}

/// Die Einträge, aus denen ein Feld wählen darf.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Offer {
    /// Das Projekt nutzt keine Versionen/Meilensteine (`use_versions` aus) oder gibt es nicht.
    Off,
    /// Die Einträge des Projekts nach Position.
    Items(Vec<Release>),
}

/// Auswahl für ein Feld im Projekt: nur die Einträge **dieses** Projekts und nur, wenn es Versionen nutzt.
pub(super) fn offer(conn: &Connection, kind: ReleaseKind, project_id: i64) -> Offer {
    match get_project(conn, project_id) {
        Ok(p) if p.use_versions => {
            let mut items = list_releases(conn, kind, project_id).unwrap_or_default();
            items.sort_by_key(|r| (r.position, r.id));
            Offer::Items(items)
        }
        _ => Offer::Off,
    }
}

/// Hinweis, wenn das Projekt keine Versionen/Meilensteine nutzt.
pub(super) fn off_text(kind: ReleaseKind, lang: Lang) -> &'static str {
    match kind {
        ReleaseKind::Version => lang.t("Versionen sind für dieses Projekt ausgeschaltet.", "Versions are turned off for this project."),
        ReleaseKind::Milestone => lang.t("Meilensteine sind für dieses Projekt ausgeschaltet.", "Milestones are turned off for this project."),
    }
}

/// Hinweis, wenn das Projekt noch keine Einträge hat.
fn none_yet_text(kind: ReleaseKind, lang: Lang) -> &'static str {
    match kind {
        ReleaseKind::Version => lang.t("(Noch keine Versionen – lege sie im Reiter „Releases“ an.)", "(No versions yet – create them in the Releases tab.)"),
        ReleaseKind::Milestone => lang.t("(Noch keine Meilensteine – lege sie im Reiter „Releases“ an.)", "(No milestones yet – create them in the Releases tab.)"),
    }
}

/// Tooltip eines Eintrags: Fälligkeit (mit „überfällig“), Zustand erschienen/erreicht und die Beschreibung.
pub(super) fn release_tooltip(r: &Release, today: chrono::NaiveDate, lang: Lang) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(d) = r.due_date {
        let mut s = format!("{} {}", lang.t("Fällig:", "Due:"), d.format("%Y-%m-%d"));
        if r.is_overdue(today) {
            s.push_str(&format!(" ({})", lang.t("überfällig", "overdue")));
        }
        parts.push(s);
    }
    if r.done {
        let at = r.done_at.as_deref().map(|a| a.chars().take(10).collect::<String>()).filter(|a| !a.is_empty());
        parts.push(match at {
            Some(a) => format!("{} {a}", r.kind.done_label(true, lang)),
            None => r.kind.done_label(true, lang).to_string(),
        });
    }
    let mut out = parts.join(" · ");
    if out.is_empty() {
        out = lang.t("Keine Fälligkeit", "No due date").to_string();
    }
    let desc = r.description.trim();
    if !desc.is_empty() {
        out.push('\n');
        out.push_str(desc);
    }
    out
}

thread_local! {
    static LABELS: RefCell<HashMap<(bool, i64), (String, Instant)>> = RefCell::new(HashMap::new());
}

/// Wie lange ein Name im Zwischenspeicher gilt (Umbenennungen erscheinen nach spätestens so langer Zeit).
const LABEL_TTL: Duration = Duration::from_secs(2);

/// Name zu einer Kennung für reine Anzeigen (Listen, Karten, Vorschauen); `#<id>`, wenn es den Eintrag nicht mehr gibt. Das
/// Ergebnis wird kurz zwischengespeichert, damit Listen nicht pro Bild und Zeile die Datenbank fragen.
pub(super) fn release_label(conn: &Connection, kind: ReleaseKind, id: i64) -> String {
    let key = (kind == ReleaseKind::Milestone, id);
    if let Some(s) = LABELS.with(|c| c.borrow().get(&key).filter(|(_, t)| t.elapsed() < LABEL_TTL).map(|(s, _)| s.clone())) {
        return s;
    }
    let name = get_release(conn, kind, id).map(|r| r.name).unwrap_or_else(|_| format!("#{id}"));
    LABELS.with(|c| c.borrow_mut().insert(key, (name.clone(), Instant::now())));
    name
}

/// Leert den Zwischenspeicher der Namen (Tests, die zwischen Datenbanken wechseln).
#[cfg(test)]
pub(super) fn clear_release_cache() {
    LABELS.with(|c| c.borrow_mut().clear());
}

/// Anzeigetext eines Terminal-Werts: Name der Sitzung, bei beendeter Sitzung mit „(beendet)“.
pub(super) fn terminal_text(target: &str, lang: Lang) -> String {
    let label = terminal_label(target, lang);
    if parse_target(target).is_some() && !matches!(terminal_link_state(target), LinkState::Running(_)) {
        format!("{label} ({})", lang.t("beendet", "ended"))
    } else {
        label
    }
}

// ------------------------------------------------------------------ Zeichnen

/// Zeile eines Eintrags in der Auswahlliste: Häkchen (erschienen/erreicht) oder leerer Kreis, Name gedämpft, wenn fertig.
fn release_row(ui: &mut Ui, r: &Release, selected: bool) -> Response {
    let color = if r.done { style::dim(ui) } else { ui.visuals().text_color() };
    let g = ui.painter().layout_no_wrap(r.name.clone(), egui::TextStyle::Button.resolve(ui.style()), color);
    let w = ui.available_width().max(g.size().x + 34.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 24.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 5.0, ui.visuals().widgets.hovered.bg_fill);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    } else if selected {
        ui.painter().rect_filled(rect, 5.0, style::accent().gamma_multiply(0.2));
    }
    let c = Pos2::new(rect.left() + 11.0, rect.center().y);
    let mark = Stroke::new(1.6_f32, style::dim(ui));
    if r.done {
        ui.painter().line_segment([c + Vec2::new(-3.5, 0.0), c + Vec2::new(-1.0, 3.0)], mark);
        ui.painter().line_segment([c + Vec2::new(-1.0, 3.0), c + Vec2::new(4.0, -3.5)], mark);
    } else {
        ui.painter().circle_stroke(c, 3.5, Stroke::new(1.0_f32, style::dim(ui)));
    }
    ui.painter().galley(Pos2::new(rect.left() + 24.0, rect.center().y - g.size().y / 2.0), g, color);
    resp
}

/// Auswahl einer Version/eines Meilensteins. `Some(neu)` bei einer Wahl (`Some(None)` = geleert), sonst `None`.
fn release_picker(ui: &mut Ui, ic: &InCx, salt: Id, kind: ReleaseKind, cur: Option<i64>, allow_clear: bool) -> Option<Option<i64>> {
    let (conn, lang) = (ic.conn, ic.lang);
    let mut chosen: Option<Option<i64>> = None;
    match offer(conn, kind, ic.project_id) {
        Offer::Off => {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(off_text(kind, lang)).color(style::dim(ui)));
                if let Some(id) = cur {
                    ui.label(format!("{} {}", lang.t("Aktueller Wert:", "Current value:"), release_label(conn, kind, id)));
                    if allow_clear && style::icon_btn_small(ui, style::Icon::Close, lang.t("Wert entfernen", "Remove the value")).clicked() {
                        chosen = Some(None);
                    }
                }
            });
        }
        Offer::Items(items) => {
            let today = crate::planning::sprints::clock::today();
            let sel = cur.and_then(|id| items.iter().find(|r| r.id == id));
            let text = match (sel, cur) {
                (Some(r), _) => r.name.clone(),
                // Der Wert gehört nicht (mehr) zu diesem Projekt: zeigen, aber nicht als Auswahl anbieten.
                (None, Some(id)) => format!("{} ({})", release_label(conn, kind, id), lang.t("nicht in diesem Projekt", "not in this project")),
                (None, None) => "–".to_string(),
            };
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt(salt).selected_text(text).width(220.0).show_ui(ui, |ui| {
                    if items.is_empty() {
                        ui.label(RichText::new(none_yet_text(kind, lang)).color(style::dim(ui)));
                    }
                    if allow_clear && ui.selectable_label(cur.is_none(), lang.t("Keine", "None")).clicked() {
                        chosen = Some(None);
                    }
                    for r in &items {
                        if release_row(ui, r, cur == Some(r.id)).on_hover_text(release_tooltip(r, today, lang)).clicked() {
                            chosen = Some(Some(r.id));
                            ui.close();
                        }
                    }
                });
                if let Some(r) = sel.filter(|r| r.done) {
                    ui.label(RichText::new(r.kind.done_label(true, lang)).small().color(style::dim(ui)));
                }
                if let Some(r) = sel {
                    if r.is_overdue(today) {
                        ui.label(RichText::new(lang.t("überfällig", "overdue")).small().color(widgets::DANGER()));
                    }
                }
            });
        }
    }
    chosen.filter(|c| *c != cur)
}

/// Chip einer Sitzung: Punkt und Name; laufend in der Akzentfarbe, beendet grau.
fn terminal_chip(ui: &mut Ui, text: &str, running: bool, tip: &str) -> Response {
    let col = if running { style::accent() } else { style::dim(ui) };
    let g = ui.painter().layout_no_wrap(text.to_string(), egui::FontId::proportional(12.5), col);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(g.size().x + 30.0, 22.0), Sense::click());
    ui.painter().rect_filled(rect, 11.0, col.gamma_multiply(0.2));
    ui.painter().circle_filled(Pos2::new(rect.left() + 11.0, rect.center().y), 3.5, col);
    ui.painter().galley(Pos2::new(rect.left() + 20.0, rect.center().y - g.size().y / 2.0), g, col);
    if running && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.on_hover_text(tip)
}

/// Eingabe eines Terminal-Werts (`session:<id>` oder leer). `Some(neu)` bei einer Änderung (`Some("")` = geleert); ein Klick
/// auf den Chip einer laufenden Sitzung setzt `jump`.
fn terminal_input(ui: &mut Ui, ic: &InCx, salt: Id, cur: &str, allow_clear: bool, jump: &mut Option<u64>) -> Option<String> {
    let lang = ic.lang;
    let edit_id = salt.with("edit");
    let mut editing: bool = ui.data(|d| d.get_temp(edit_id)).unwrap_or(false);
    let mut out: Option<String> = None;
    let has = !cur.trim().is_empty();
    ui.horizontal(|ui| {
        if has {
            let state = terminal_link_state(cur);
            let running = matches!(state, LinkState::Running(_));
            let tip = if running {
                lang.t("Zur Sitzung springen", "Jump to the session")
            } else if parse_target(cur).is_some() {
                lang.t("Die Sitzung ist beendet.", "The session has ended.")
            } else {
                lang.t("Kein Verweis auf eine Sitzung dieser Anwendung.", "Not a reference to a session of this application.")
            };
            if terminal_chip(ui, &terminal_text(cur, lang), running, tip).clicked() {
                if let LinkState::Running(id) = state {
                    *jump = Some(id);
                }
            }
            if ui.small_button(lang.t("Ändern", "Change")).on_hover_text(lang.t("Eine andere Sitzung wählen", "Pick another session")).clicked() {
                editing = !editing;
            }
            if allow_clear && style::icon_btn_small(ui, style::Icon::Close, lang.t("Verweis entfernen", "Remove the reference")).clicked() {
                out = Some(String::new());
                editing = false;
            }
        }
        if editing || !has {
            if let Some(new) = pick_terminal(ui, salt, cur, lang) {
                // Ein Pflichtfeld lässt sich nicht leeren (wie beim Dropdown).
                if !new.is_empty() || allow_clear {
                    out = Some(new);
                }
                editing = false;
            }
        }
    });
    ui.data_mut(|d| d.insert_temp(edit_id, editing));
    out
}

/// Eingabe für ein Feld der Art Version, Meilenstein oder Terminal auf einem JSON-Wert (Kennung bzw. `session:<id>`-Text,
/// `null`/`""` = leer). `true`, wenn der Nutzer etwas geändert hat (der Aufrufer speichert). `allow_clear`: leeren erlaubt.
/// `jump`: Sitzung, zu der der Nutzer springen will.
pub(super) fn ref_value_editor(ui: &mut Ui, ic: &InCx, salt: Id, kind: FieldKind, v: &mut Value, allow_clear: bool, jump: &mut Option<u64>) -> bool {
    match kind {
        FieldKind::Version | FieldKind::Milestone => {
            let Some(rk) = release_kind(kind) else { return false };
            let cur = v.as_i64().filter(|i| *i > 0);
            match release_picker(ui, ic, salt, rk, cur, allow_clear) {
                Some(new) => {
                    *v = new.map(|i| json!(i)).unwrap_or(Value::Null);
                    true
                }
                None => false,
            }
        }
        FieldKind::Terminal => {
            let cur = v.as_str().unwrap_or("").to_string();
            match terminal_input(ui, ic, salt, &cur, allow_clear, jump) {
                Some(new) => {
                    *v = Value::String(new);
                    true
                }
                None => false,
            }
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::projects::{create_project, NewProject, UpdateProject};
    use crate::planning::sprints::releases::{create_milestone, create_version, reorder_versions, set_version_released, update_version, NewRelease, UpdateRelease};
    use crate::planning::testutil::db;
    use chrono::NaiveDate;

    fn date(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn project(c: &Connection, key: &str, versions: bool) -> i64 {
        let mut np = NewProject::new(key, key, "#445566");
        np.use_versions = versions;
        create_project(c, &np).unwrap()
    }

    #[test]
    fn field_kinds_map_to_release_kinds() {
        assert_eq!(release_kind(FieldKind::Version), Some(ReleaseKind::Version));
        assert_eq!(release_kind(FieldKind::Milestone), Some(ReleaseKind::Milestone));
        assert_eq!(release_kind(FieldKind::Terminal), None);
        assert_eq!(release_kind(FieldKind::Dropdown), None);
    }

    #[test]
    fn offers_only_entries_of_the_project_in_position_order() {
        let c = db();
        let p = project(&c, "AA", true);
        let q = project(&c, "BB", true);
        let a = create_version(&c, &NewRelease::new(p, "1.0")).unwrap();
        let b = create_version(&c, &NewRelease::new(p, "2.0")).unwrap();
        let other = create_version(&c, &NewRelease::new(q, "9.9")).unwrap();
        create_milestone(&c, &NewRelease::new(p, "Beta")).unwrap();
        let ids = |o: Offer| match o {
            Offer::Items(v) => v.into_iter().map(|r| r.id).collect::<Vec<_>>(),
            Offer::Off => panic!("sollte Einträge liefern"),
        };
        assert_eq!(ids(offer(&c, ReleaseKind::Version, p)), vec![a, b]);
        assert_eq!(ids(offer(&c, ReleaseKind::Version, q)), vec![other], "nur die Versionen des eigenen Projekts");
        assert_eq!(ids(offer(&c, ReleaseKind::Milestone, q)), Vec::<i64>::new());
        reorder_versions(&c, p, &[b, a]).unwrap();
        assert_eq!(ids(offer(&c, ReleaseKind::Version, p)), vec![b, a], "nach der eingestellten Reihenfolge");
    }

    #[test]
    fn nothing_is_offered_when_versions_are_off_or_the_project_is_unknown() {
        let c = db();
        let p = project(&c, "AA", true);
        create_version(&c, &NewRelease::new(p, "1.0")).unwrap();
        crate::planning::projects::update_project(&c, p, &UpdateProject { use_versions: Some(false), ..Default::default() }).unwrap();
        assert_eq!(offer(&c, ReleaseKind::Version, p), Offer::Off);
        assert_eq!(offer(&c, ReleaseKind::Milestone, 9999), Offer::Off);
        assert!(off_text(ReleaseKind::Version, Lang::De).contains("ausgeschaltet"));
        assert!(off_text(ReleaseKind::Milestone, Lang::En).contains("turned off"));
        assert!(off_text(ReleaseKind::Version, Lang::De).starts_with("Versionen"));
        assert!(off_text(ReleaseKind::Milestone, Lang::De).starts_with("Meilensteine"));
    }

    #[test]
    fn tooltips_name_due_date_state_and_description() {
        let c = db();
        let p = project(&c, "AA", true);
        let id = create_version(&c, &NewRelease { due_date: Some(date("2026-11-01")), description: "Erstes Release".into(), ..NewRelease::new(p, "1.0") }).unwrap();
        let r = crate::planning::sprints::releases::get_version(&c, id).unwrap();
        let today = date("2026-11-14");
        let t = release_tooltip(&r, today, Lang::En);
        assert!(t.contains("Due: 2026-11-01") && t.contains("overdue") && t.contains("Erstes Release"), "{t}");
        assert!(release_tooltip(&r, today, Lang::De).contains("Fällig: 2026-11-01 (überfällig)"));
        set_version_released(&c, id, true).unwrap();
        let r = crate::planning::sprints::releases::get_version(&c, id).unwrap();
        let t = release_tooltip(&r, today, Lang::En);
        assert!(t.contains("Released") && !t.contains("overdue"), "erschienen ist nicht überfällig: {t}");
        assert!(release_tooltip(&r, today, Lang::De).contains("Erschienen"));
        let plain = create_milestone(&c, &NewRelease::new(p, "M")).unwrap();
        let m = crate::planning::sprints::releases::get_milestone(&c, plain).unwrap();
        assert_eq!(release_tooltip(&m, today, Lang::En), "No due date");
        assert_eq!(release_tooltip(&m, today, Lang::De), "Keine Fälligkeit");
    }

    #[test]
    fn labels_show_the_name_and_fall_back_to_the_id() {
        let c = db();
        clear_release_cache();
        let p = project(&c, "AA", true);
        let v = create_version(&c, &NewRelease::new(p, "Frühling")).unwrap();
        let m = create_milestone(&c, &NewRelease::new(p, "Beta")).unwrap();
        assert_eq!(release_label(&c, ReleaseKind::Version, v), "Frühling");
        assert_eq!(release_label(&c, ReleaseKind::Milestone, m), "Beta");
        assert_eq!(release_label(&c, ReleaseKind::Version, 4242), "#4242");
        // der Zwischenspeicher hält den alten Namen, bis er geleert wird (oder abläuft)
        update_version(&c, v, &UpdateRelease { name: Some("Sommer".into()), ..Default::default() }).unwrap();
        assert_eq!(release_label(&c, ReleaseKind::Version, v), "Frühling");
        clear_release_cache();
        assert_eq!(release_label(&c, ReleaseKind::Version, v), "Sommer");
        // Version und Meilenstein mit gleicher Kennung stören sich nicht
        assert_eq!(release_label(&c, ReleaseKind::Milestone, v), "Beta");
        clear_release_cache();
    }
}
