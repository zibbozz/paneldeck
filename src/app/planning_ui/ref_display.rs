//! Anzeige der Feldarten Version, Meilenstein und Terminal **ohne Eingabe**: Board-Karten, Ticket-Liste und die Verknüpfungen
//! in der Ticket-Ansicht. Die Eingaben stehen in `ref_inputs.rs`; hier steckt nur, was Anzeigen gemeinsam brauchen.
//!
//! * [`RefNames`] holt die Namen aller Versionen/Meilensteine eines Projekts mit **zwei** Abfragen (statt einer je Karte/Zeile);
//!   Kennungen aus anderen Projekten fallen auf [`release_label`] (mit kleinem Zwischenspeicher) zurück.
//! * [`ref_text`] macht aus dem Wert eines Felds den Anzeigetext; bei Terminal-Werten zusätzlich „gedämpft“ (Sitzung beendet).
//! * [`load_row_refs`] lädt die Werte der drei Feldarten für **alle** Tickets des Projekts mit einer Abfrage; die Ticket-Liste
//!   zeichnet daraus Chips ([`row_chips`], [`paint_chips`]) – ohne im Zeichencode die Datenbank zu fragen.
//! * [`scroll_target`] ist die reine Rechnung, mit der die Liste zum geöffneten Ticket scrollt.

use super::ref_inputs::{release_kind, release_label, terminal_text};
use super::super::terminal_links::{terminal_link_state, LinkState};
use crate::planning::fields::FieldKind;
use crate::planning::sprints::releases::{list_releases, ReleaseKind};
use crate::state::Lang;
use crate::style;
use eframe::egui::{self, Color32, FontId, Painter, Pos2, Rect, Stroke, Ui, Vec2};
use rusqlite::Connection;
use serde_json::Value;
use std::collections::HashMap;

// ------------------------------------------------------------------ Namen

/// Namen der Versionen und Meilensteine eines Projekts (einmal geladen, dann nur nachgeschlagen).
#[derive(Debug, Default)]
pub struct RefNames {
    names: HashMap<(bool, i64), String>,
}

impl RefNames {
    /// Lädt alle Versionen und Meilensteine des Projekts (zwei Abfragen).
    pub fn load(conn: &Connection, project_id: i64) -> RefNames {
        let mut names = HashMap::new();
        for kind in [ReleaseKind::Version, ReleaseKind::Milestone] {
            for r in list_releases(conn, kind, project_id).unwrap_or_default() {
                names.insert((kind == ReleaseKind::Milestone, r.id), r.name);
            }
        }
        RefNames { names }
    }

    /// Name zur Kennung; gehört sie nicht zum Projekt (oder gibt es sie nicht mehr), fragt [`release_label`] (mit Zwischenspeicher).
    pub fn name(&self, conn: &Connection, kind: ReleaseKind, id: i64) -> String {
        self.names.get(&(kind == ReleaseKind::Milestone, id)).cloned().unwrap_or_else(|| release_label(conn, kind, id))
    }
}

/// Anzeigetext des Werts eines Version-, Meilenstein- oder Terminal-Felds und ob er gedämpft erscheint (beendete Sitzung).
/// `None` für andere Feldarten und leere Werte.
pub fn ref_text(names: &RefNames, conn: &Connection, kind: FieldKind, v: &Value, lang: Lang) -> Option<(String, bool)> {
    match kind {
        FieldKind::Version | FieldKind::Milestone => {
            let id = v.as_i64().filter(|i| *i > 0)?;
            Some((names.name(conn, release_kind(kind)?, id), false))
        }
        FieldKind::Terminal => {
            let s = v.as_str()?.trim();
            if s.is_empty() {
                return None;
            }
            let running = matches!(terminal_link_state(s), LinkState::Running(_));
            Some((terminal_text(s, lang), !running))
        }
        _ => None,
    }
}

/// Anzeige einer Terminal-Verknüpfung in der Ticket-Ansicht: Text mit Namen der Sitzung (bei beendeter „beendet“), die Kennung der
/// laufenden Sitzung (dann ist der Eintrag klickbar und springt dorthin) und der Tooltip.
pub fn terminal_link_view(target: &str, lang: Lang) -> (String, Option<u64>, &'static str) {
    let text = terminal_text(target, lang);
    match terminal_link_state(target) {
        LinkState::Running(id) => (text, Some(id), lang.t("Zur Sitzung springen", "Jump to the session")),
        LinkState::Ended if super::super::terminal_links::parse_target(target).is_some() => (text, None, lang.t("Die Sitzung ist beendet.", "The session has ended.")),
        LinkState::Ended => (text, None, lang.t("Kein Verweis auf eine Sitzung dieser Anwendung.", "Not a reference to a session of this application.")),
    }
}

// ------------------------------------------------------------------ Ticket-Liste

/// Version, Meilenstein und Terminal eines Tickets für die Liste (Namen sind schon aufgelöst, der Terminal-Wert noch roh,
/// weil sich der Zustand der Sitzung ändert).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RowRefs {
    pub version: Option<String>,
    pub milestone: Option<String>,
    /// Roher Wert (`session:<id>`).
    pub terminal: Option<String>,
}

/// Die Werte der Feldarten Version, Meilenstein und Terminal aller Tickets des Projekts (nur Tickets mit mindestens einem Wert).
/// Hat ein Ticket mehrere Felder derselben Art, gilt das erste.
pub fn load_row_refs(conn: &Connection, project_id: i64) -> HashMap<i64, RowRefs> {
    let mut out: HashMap<i64, RowRefs> = HashMap::new();
    let Ok(mut st) = conn.prepare(
        "SELECT v.ticket_id, f.kind, v.text FROM ticket_values v JOIN fields f ON f.id = v.field_id JOIN tickets t ON t.id = v.ticket_id
         WHERE t.project_id = ?1 AND f.kind IN ('version', 'milestone', 'terminal') AND v.text IS NOT NULL AND v.text <> ''
         ORDER BY v.ticket_id, v.field_id, v.pos",
    ) else {
        return out;
    };
    let Ok(rows) = st.query_map([project_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))) else { return out };
    let names = RefNames::load(conn, project_id);
    for (ticket, kind, text) in rows.flatten() {
        let e = out.entry(ticket).or_default();
        let id = text.trim().parse::<i64>().ok().filter(|i| *i > 0);
        match (kind.as_str(), id) {
            ("version", Some(id)) if e.version.is_none() => e.version = Some(names.name(conn, ReleaseKind::Version, id)),
            ("milestone", Some(id)) if e.milestone.is_none() => e.milestone = Some(names.name(conn, ReleaseKind::Milestone, id)),
            ("terminal", _) if e.terminal.is_none() => e.terminal = Some(text.trim().to_string()),
            _ => {}
        }
    }
    out.retain(|_, r| *r != RowRefs::default());
    out
}

/// Art eines Chips in der Liste (bestimmt die Farbe).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ChipKind {
    Version,
    Milestone,
    Terminal,
}

/// Ein Chip der Listenzeile.
#[derive(Debug, Clone, PartialEq)]
pub struct RefChip {
    pub kind: ChipKind,
    pub text: String,
    /// Gedämpft (beendete Sitzung).
    pub muted: bool,
}

/// Die Chips einer Zeile in fester Reihenfolge: Version, Meilenstein, Terminal.
pub fn row_chips(r: &RowRefs, lang: Lang) -> Vec<RefChip> {
    let mut out = Vec::new();
    if let Some(v) = &r.version {
        out.push(RefChip { kind: ChipKind::Version, text: v.clone(), muted: false });
    }
    if let Some(m) = &r.milestone {
        out.push(RefChip { kind: ChipKind::Milestone, text: m.clone(), muted: false });
    }
    if let Some(t) = &r.terminal {
        let running = matches!(terminal_link_state(t), LinkState::Running(_));
        out.push(RefChip { kind: ChipKind::Terminal, text: format!("⌨ {}", terminal_text(t, lang)), muted: !running });
    }
    out
}

/// Zeilen „Version: 1.0“ usw. für den Tooltip einer Listenzeile.
pub fn row_tooltip(r: &RowRefs, lang: Lang) -> String {
    let mut lines: Vec<String> = Vec::new();
    if let Some(v) = &r.version {
        lines.push(format!("{}: {v}", lang.t("Version", "Version")));
    }
    if let Some(m) = &r.milestone {
        lines.push(format!("{}: {m}", lang.t("Meilenstein", "Milestone")));
    }
    if let Some(t) = &r.terminal {
        lines.push(format!("{}: {}", lang.t("Terminal", "Terminal"), terminal_text(t, lang)));
    }
    lines.join("\n")
}

/// Farbe eines Chips.
fn chip_color(ui: &Ui, c: &RefChip) -> Color32 {
    if c.muted {
        return style::dim(ui);
    }
    match c.kind {
        ChipKind::Version => style::accent(),
        ChipKind::Milestone => Color32::from_rgb(0xc6, 0x78, 0xdd),
        ChipKind::Terminal => Color32::from_rgb(0x98, 0xc3, 0x79),
    }
}

/// Zeichnet die Chips nebeneinander in die Zelle; was nicht mehr passt, wird gekürzt und durch „…“ ersetzt.
pub fn paint_chips(ui: &Ui, p: &Painter, cell: Rect, chips: &[RefChip]) {
    let mut x = cell.left();
    for c in chips {
        let col = chip_color(ui, c);
        let avail = cell.right() - x;
        if avail < 34.0 {
            let g = p.layout_no_wrap("…".into(), FontId::proportional(11.5), style::dim(ui));
            p.galley(Pos2::new(x, cell.center().y - g.size().y / 2.0), g, style::dim(ui));
            break;
        }
        let mut job = egui::text::LayoutJob::simple_singleline(c.text.clone(), FontId::proportional(11.5), col);
        job.wrap.max_width = avail - 14.0;
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        job.wrap.overflow_character = Some('…');
        let g = p.layout_job(job);
        let r = Rect::from_min_size(Pos2::new(x, cell.center().y - 9.0), Vec2::new(g.size().x + 14.0, 18.0));
        p.rect_filled(r, 9.0, col.gamma_multiply(0.18));
        if c.muted {
            p.rect_stroke(r, 9.0, Stroke::new(1.0_f32, col.gamma_multiply(0.35)), egui::StrokeKind::Inside);
        }
        p.galley(Pos2::new(r.left() + 7.0, r.center().y - g.size().y / 2.0), g, col);
        x = r.right() + 4.0;
    }
}

// ------------------------------------------------------------------ Scrollen

/// Neue Scroll-Position, damit die Zeile `index` in der Liste sichtbar ist; `None`, wenn sie schon ganz zu sehen ist (dann
/// bleibt die Position, wie sie ist). Die Zeile wird in die Mitte gerückt und die Position auf den gültigen Bereich begrenzt.
pub fn scroll_target(index: usize, row_h: f32, offset: f32, view_h: f32, content_h: f32) -> Option<f32> {
    let top = index as f32 * row_h;
    let bottom = top + row_h;
    if top >= offset - 0.5 && bottom <= offset + view_h + 0.5 {
        return None;
    }
    let max = (content_h - view_h).max(0.0);
    Some((top + row_h / 2.0 - view_h / 2.0).clamp(0.0, max))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::terminal_links::{set_live_for_test, LiveSession};
    use crate::planning::fields::{create_field, NewField};
    use crate::planning::history::Source;
    use crate::planning::projects::{create_project, NewProject};
    use crate::planning::sprints::releases::{create_milestone, create_version, NewRelease};
    use crate::planning::tickets::fixture::Fx;
    use crate::planning::tickets::{create_ticket, NewTicket};
    use serde_json::json;

    #[test]
    fn scroll_target_leaves_visible_rows_alone_and_centres_the_others() {
        // 100 Zeilen à 30, Ausschnitt 300 hoch
        assert_eq!(scroll_target(3, 30.0, 0.0, 300.0, 3000.0), None, "ganz sichtbar");
        assert_eq!(scroll_target(9, 30.0, 0.0, 300.0, 3000.0), None, "letzte ganz sichtbare Zeile");
        assert_eq!(scroll_target(10, 30.0, 0.0, 300.0, 3000.0), Some(165.0), "knapp außerhalb: Mitte");
        assert_eq!(scroll_target(50, 30.0, 0.0, 300.0, 3000.0), Some(1365.0));
        assert_eq!(scroll_target(0, 30.0, 600.0, 300.0, 3000.0), Some(0.0), "oben begrenzt");
        assert_eq!(scroll_target(99, 30.0, 0.0, 300.0, 3000.0), Some(2700.0), "unten begrenzt");
        assert_eq!(scroll_target(5, 30.0, 135.0, 300.0, 3000.0), None, "5 beginnt bei 150");
        assert_eq!(scroll_target(4, 30.0, 135.0, 300.0, 3000.0), Some(0.0), "teilweise verdeckt gilt als nicht sichtbar");
        assert_eq!(scroll_target(2, 30.0, 0.0, 300.0, 100.0), None, "Inhalt kleiner als der Ausschnitt");
    }

    #[test]
    fn names_are_loaded_once_per_project_and_fall_back_for_foreign_ids() {
        let f = Fx::new();
        crate::planning::projects::update_project(&f.c, f.project, &crate::planning::projects::UpdateProject { use_versions: Some(true), ..Default::default() }).unwrap();
        let mut np = NewProject::new("ZZ", "Zwei", "#112233");
        np.use_versions = true;
        let other = create_project(&f.c, &np).unwrap();
        let v = create_version(&f.c, &NewRelease::new(f.project, "Frühling")).unwrap();
        let m = create_milestone(&f.c, &NewRelease::new(f.project, "Beta")).unwrap();
        let foreign = create_version(&f.c, &NewRelease::new(other, "Fremd")).unwrap();
        super::super::ref_inputs::clear_release_cache();
        let names = RefNames::load(&f.c, f.project);
        assert_eq!(names.name(&f.c, ReleaseKind::Version, v), "Frühling");
        assert_eq!(names.name(&f.c, ReleaseKind::Milestone, m), "Beta");
        assert_eq!(names.name(&f.c, ReleaseKind::Version, foreign), "Fremd", "fremde Kennung über den Zwischenspeicher");
        assert_eq!(names.name(&f.c, ReleaseKind::Version, 9999), "#9999");
    }

    #[test]
    fn ref_text_covers_the_three_kinds_and_ignores_empty_values() {
        let _live = crate::app::terminal_links::live_test_guard();
        let f = Fx::new();
        crate::planning::projects::update_project(&f.c, f.project, &crate::planning::projects::UpdateProject { use_versions: Some(true), ..Default::default() }).unwrap();
        let v = create_version(&f.c, &NewRelease::new(f.project, "1.0")).unwrap();
        let m = create_milestone(&f.c, &NewRelease::new(f.project, "Beta")).unwrap();
        let names = RefNames::load(&f.c, f.project);
        set_live_for_test(vec![
            LiveSession { id: 7, name: "Build".into(), folder: String::new(), running: true },
            LiveSession { id: 8, name: "Old".into(), folder: String::new(), running: false },
        ]);
        let t = |k, val: Value, l| ref_text(&names, &f.c, k, &val, l);
        assert_eq!(t(FieldKind::Version, json!(v), Lang::En), Some(("1.0".into(), false)));
        assert_eq!(t(FieldKind::Milestone, json!(m), Lang::De), Some(("Beta".into(), false)));
        assert_eq!(t(FieldKind::Version, Value::Null, Lang::En), None);
        assert_eq!(t(FieldKind::Version, json!(0), Lang::En), None);
        assert_eq!(t(FieldKind::Terminal, json!("session:7"), Lang::En), Some(("Build".into(), false)));
        assert_eq!(t(FieldKind::Terminal, json!("session:8"), Lang::En), Some(("Old (ended)".into(), true)), "beendet: gedämpft");
        assert_eq!(t(FieldKind::Terminal, json!("session:8"), Lang::De), Some(("Old (beendet)".into(), true)));
        assert_eq!(t(FieldKind::Terminal, json!("  "), Lang::En), None);
        assert_eq!(t(FieldKind::Text, json!("x"), Lang::En), None);
        set_live_for_test(Vec::new());
    }

    #[test]
    fn row_refs_are_loaded_in_one_go_and_turned_into_chips_and_tooltips() {
        let _live = crate::app::terminal_links::live_test_guard();
        let f = Fx::new();
        crate::planning::projects::update_project(&f.c, f.project, &crate::planning::projects::UpdateProject { use_versions: Some(true), ..Default::default() }).unwrap();
        let vf = create_field(&f.c, &NewField::new(f.task, "Fix version", FieldKind::Version)).unwrap();
        let mf = create_field(&f.c, &NewField::new(f.task, "Target milestone", FieldKind::Milestone)).unwrap();
        let tf = create_field(&f.c, &NewField::new(f.task, "Session", FieldKind::Terminal)).unwrap();
        let v = create_version(&f.c, &NewRelease::new(f.project, "1.0")).unwrap();
        let m = create_milestone(&f.c, &NewRelease::new(f.project, "Beta")).unwrap();
        let full = create_ticket(&f.c, &NewTicket { values: vec![(vf, json!(v)), (mf, json!(m)), (tf, json!("session:7"))], ..NewTicket::new(f.project, f.task, "Voll") }, &Source::ui()).unwrap().0;
        let bare = f.mk(f.task, "Leer");
        set_live_for_test(vec![LiveSession { id: 7, name: "Build".into(), folder: String::new(), running: true }]);
        let all = load_row_refs(&f.c, f.project);
        assert_eq!(all.len(), 1, "Tickets ohne Werte tauchen nicht auf");
        assert!(!all.contains_key(&bare.id));
        let r = &all[&full.id];
        assert_eq!(r, &RowRefs { version: Some("1.0".into()), milestone: Some("Beta".into()), terminal: Some("session:7".into()) });
        let chips = row_chips(r, Lang::En);
        assert_eq!(chips.iter().map(|c| c.kind).collect::<Vec<_>>(), vec![ChipKind::Version, ChipKind::Milestone, ChipKind::Terminal]);
        assert_eq!(chips[2].text, "⌨ Build");
        assert!(chips.iter().all(|c| !c.muted));
        assert_eq!(row_tooltip(r, Lang::En), "Version: 1.0\nMilestone: Beta\nTerminal: Build");
        assert_eq!(row_tooltip(r, Lang::De), "Version: 1.0\nMeilenstein: Beta\nTerminal: Build");
        // Sitzung beendet: grauer Chip mit Hinweis
        set_live_for_test(Vec::new());
        let chips = row_chips(r, Lang::En);
        assert!(chips[2].muted && chips[2].text.contains("ended"), "{:?}", chips[2]);
        assert!(row_chips(&RowRefs::default(), Lang::En).is_empty());
        assert_eq!(row_tooltip(&RowRefs::default(), Lang::En), "");
        assert!(load_row_refs(&f.c, 9999).is_empty());
    }
}
