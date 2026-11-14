//! Ticket↔Terminal-Verknüpfung in der Oberfläche (Terminal Hub): Mini-Suche zum Verknüpfen, Chips mit der Ticket-Kennung im
//! Terminalkopf und in der Seitenleiste, Ticket-Treffer in der Befehls-Palette und der Auswahldialog für Felder vom Typ
//! „Terminal“. Die Daten liegen in `crate::planning::links` (System-Verknüpfungstyp `terminal`).
//!
//! * **Ziel-Format:** eine Sitzung wird als `session:<id>` gespeichert (`<id>` = `SessionMeta::id`, bleibt über `state.json`
//!   und Neustarts erhalten und wird nie wiederverwendet). [`target_of`] baut es, [`parse_target`] liest es. Dasselbe Format
//!   gilt für Werte von Feldern des Typs „Terminal“ und für künftige MCP-Werkzeuge (`link_terminal`).
//! * **Zustand „beendet“:** Die Verknüpfung bleibt in der Datenbank, wenn die Sitzung geschlossen wird. Ob sie noch lebt,
//!   steht in der Anwendung, nicht in der Datenbank: die App veröffentlicht jedes Bild die Liste der Sitzungen
//!   (`publish_live`). [`terminal_link_state`] fragt sie ab – `Running(id)` = Sitzung existiert und läuft, `Ended` = sie
//!   existiert nicht mehr, ist beendet oder das Ziel ist kein `session:<id>`. Die Ticket-Ansicht zeigt bei `Running(id)` den
//!   Namen ([`terminal_label`]) und springt per [`App::open_session_from_ticket`] zur Sitzung, bei `Ended` grau „beendet“.
//!   Wird die Sitzung neu gestartet oder die App neu gestartet (gleiche ID), ist die Verknüpfung automatisch wieder aktiv.
//! * **Sichtbarkeit:** Alles hier erscheint nur, wenn der Expertenmodus an ist und es mindestens ein Experten-Projekt gibt
//!   (das schreibgeschützte Todolist-Projekt zählt nicht); sonst bleibt der Terminal Hub unverändert.
//! * **Ticket öffnen:** [`App::open_ticket_from_anywhere`] schaltet auf den Todolist-Reiter und öffnet das Ticket in der Ticket-Ansicht
//!   (Projektwechsel, Expertenmodus bei Bedarf); die Navigation steht in `planning_ui/ticket_nav.rs`.

use super::*;
use crate::planning::history::Source;
use crate::planning::link_types::link_type_by_key;
use crate::planning::links::{add_link, remove_link, Target};
use crate::planning::projects::list_projects;
use crate::planning::statuses::get_status;
use crate::planning::ticket_list::{list_tickets, TicketFilter, TicketOrder};
use crate::planning::tickets::get_ticket;
use crate::planning::types::get_type;
use crate::planning::ticket_error::TicketError;
use egui::Popup;
use rusqlite::Connection;
use std::sync::Mutex;

// ------------------------------------------------------------------ Ziel-Format und Zustand (rein, testbar)

const PREFIX: &str = "session:";

/// Ziel-Text einer Sitzung in der Verknüpfung (`session:<id>`).
pub fn target_of(session: u64) -> String {
    format!("{PREFIX}{session}")
}

/// Sitzungs-Kennung aus einem Ziel-Text; `None`, wenn es kein `session:<id>` ist.
pub fn parse_target(s: &str) -> Option<u64> {
    let n = s.trim().strip_prefix(PREFIX)?;
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    n.parse().ok()
}

/// Zustand einer Terminal-Verknüpfung.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    /// Die Sitzung existiert und läuft (Kennung zum Hinspringen).
    Running(u64),
    /// Die Sitzung existiert nicht mehr oder ist beendet.
    Ended,
}

/// Eine Sitzung, wie sie die App veröffentlicht.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveSession {
    pub id: u64,
    pub name: String,
    /// Ordner/Gruppe der Sitzung (sonst der Arbeitsordner, nur der letzte Teil).
    pub folder: String,
    /// Der Prozess läuft noch.
    pub running: bool,
}

static LIVE: Mutex<Vec<LiveSession>> = Mutex::new(Vec::new());

fn live() -> Vec<LiveSession> {
    LIVE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Gewünschter Sprung zu einer Sitzung (aus der Ticket-Ansicht): die Oberfläche der Todo-Liste kennt die Anwendung nicht und
/// legt den Wunsch hier ab; `App::tlinks_frame` holt ihn im nächsten Bild ab.
static JUMP: Mutex<Option<u64>> = Mutex::new(None);

/// Merkt den Sprung zur Sitzung `session` (Klick auf den Terminal-Chip eines Felds).
pub fn request_jump(session: u64) {
    *JUMP.lock().unwrap_or_else(|e| e.into_inner()) = Some(session);
}

/// Holt den gemerkten Sprung ab (einmalig).
pub fn take_jump() -> Option<u64> {
    JUMP.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Nur für Tests: setzt die Liste der Sitzungen, wie sie die Anwendung jedes Bild veröffentlicht.
#[cfg(test)]
pub fn set_live_for_test(sessions: Vec<LiveSession>) {
    *LIVE.lock().unwrap_or_else(|e| e.into_inner()) = sessions;
}

/// Nur für Tests: Sperre für alle Tests, die die Liste der Sitzungen setzen und danach lesen (sie ist ein gemeinsamer Zustand,
/// die Tests laufen parallel). Die Sperre bis zum Ende des Tests halten.
#[cfg(test)]
pub fn live_test_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Zustand der Verknüpfung mit diesem Ziel (siehe Modul-Doku). Nur im UI-Thread der App sinnvoll.
#[allow(dead_code)] // API für die Ticket-Ansicht
pub fn terminal_link_state(target: &str) -> LinkState {
    state_in(&live(), target)
}

fn state_in(live: &[LiveSession], target: &str) -> LinkState {
    match parse_target(target).and_then(|id| live.iter().find(|s| s.id == id)) {
        Some(s) if s.running => LinkState::Running(s.id),
        _ => LinkState::Ended,
    }
}

/// Anzeigename des Ziels: Name der Sitzung, solange sie existiert, sonst „Terminal 12“ bzw. der rohe Text.
#[allow(dead_code)] // API für die Ticket-Ansicht
pub fn terminal_label(target: &str, lang: Lang) -> String {
    match parse_target(target) {
        Some(id) => live().iter().find(|s| s.id == id).map(|s| s.name.clone()).unwrap_or_else(|| format!("{} {id}", lang.t("Terminal", "Terminal"))),
        None => target.trim().to_string(),
    }
}

/// Ticket-Schlüssel aus einer Eingabe: `pan423`, `PAN-423`, `pan-423`, `pan 423` → `PAN-423`; auch Teile (`pan-4` → `PAN-4`).
/// `None`, wenn die Eingabe nicht wie ein Schlüssel aussieht (Buchstaben, dann Ziffern).
pub fn normalize_key_query(q: &str) -> Option<String> {
    let t: String = q.chars().filter(|c| !c.is_whitespace()).collect();
    let digits = t.chars().rev().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 || digits == t.len() {
        return None;
    }
    let (pre, num) = t.split_at(t.len() - digits);
    let pre = pre.strip_suffix('-').unwrap_or(pre);
    let mut chars = pre.chars();
    if !chars.next().is_some_and(|c| c.is_ascii_alphabetic()) || !pre.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let num = num.trim_start_matches('0');
    Some(format!("{}-{}", pre.to_ascii_uppercase(), if num.is_empty() { "0" } else { num }))
}

/// Wie gut ein Ticket zur Eingabe passt (0 = gar nicht): genauer Schlüssel vor Schlüssel-Anfang vor Titel-Anfang vor
/// Schlüssel-/Titel-Teil.
pub fn relevance(query: &str, key: &str, title: &str) -> u32 {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return 1;
    }
    let (k, t) = (key.to_lowercase(), title.to_lowercase());
    if let Some(c) = normalize_key_query(&q).map(|c| c.to_lowercase()) {
        if k == c {
            return 100;
        }
        if k.starts_with(&c) {
            return 80;
        }
    }
    if k == q {
        return 100;
    }
    if t == q {
        return 70;
    }
    if t.starts_with(&q) {
        return 60;
    }
    if k.contains(&q) {
        return 50;
    }
    if t.contains(&q) {
        return 40;
    }
    let words: Vec<&str> = q.split_whitespace().collect();
    if !words.is_empty() && words.iter().all(|w| t.contains(w) || k.contains(w)) {
        return 30;
    }
    10
}

/// Beschriftung des Chips: ein Ticket → `PAN-1`, mehrere → `PAN-1 +2`.
pub fn chip_label(keys: &[&str]) -> String {
    match keys {
        [] => String::new(),
        [one] => one.to_string(),
        [first, rest @ ..] => format!("{first} +{}", rest.len()),
    }
}

// ------------------------------------------------------------------ Daten

/// Ein verknüpftes Ticket, wie der Chip es zeigt.
#[derive(Clone, Debug)]
pub struct Chip {
    pub ticket_id: i64,
    pub key: String,
    pub title: String,
    pub status: String,
    /// Farbe des Ticket-Typs.
    pub color: Color32,
    /// Icon-Name des Typs (`planning::icons`).
    pub icon: Option<String>,
}

/// Ein Suchtreffer (Mini-Suche und Palette).
#[derive(Clone, Debug)]
struct Hit {
    chip: Chip,
    status_color: Color32,
    /// Das Ticket ist mit der Sitzung schon verknüpft.
    linked: bool,
}

struct Picker {
    session: u64,
    query: String,
    searched: Option<String>,
    hits: Vec<Hit>,
    sel: usize,
}

/// Zustand der Terminal-Verknüpfungen in der Oberfläche.
#[derive(Default)]
pub struct TLinks {
    /// Die Funktion ist sichtbar (Expertenmodus an und mindestens ein Experten-Projekt).
    pub(super) enabled: bool,
    chips: HashMap<u64, Vec<Chip>>,
    stamp: Option<Instant>,
    dirty: bool,
    picker: Option<Picker>,
    pal: Option<(String, Instant, Vec<Hit>)>,
}

impl TLinks {
    /// Chips der Sitzung (leer, wenn die Funktion aus ist).
    pub(super) fn chips_of(&self, session: u64) -> &[Chip] {
        if self.enabled { self.chips.get(&session).map(Vec::as_slice).unwrap_or(&[]) } else { &[] }
    }
}

fn has_expert_project(conn: &Connection) -> bool {
    list_projects(conn).map(|v| v.iter().any(|p| !p.system)).unwrap_or(false)
}

fn chip_of(conn: &Connection, ticket: &crate::planning::tickets::Ticket, lang: Lang, cache: &mut Cache) -> (Chip, Color32) {
    let ty = cache.types.entry(ticket.type_id).or_insert_with(|| {
        get_type(conn, ticket.type_id).map(|t| (planning_ui::color_of(&t.color), t.icon.clone())).unwrap_or((Color32::GRAY, None))
    });
    let (color, icon) = (ty.0, ty.1.clone());
    let st = match ticket.status_id {
        Some(sid) => cache
            .statuses
            .entry(sid)
            .or_insert_with(|| get_status(conn, sid).map(|s| (s.display(lang), planning_ui::color_of(&s.color))).unwrap_or((String::new(), Color32::GRAY)))
            .clone(),
        None => (String::new(), Color32::GRAY),
    };
    (Chip { ticket_id: ticket.id, key: ticket.key(), title: ticket.title.clone(), status: st.0, color, icon }, st.1)
}

#[derive(Default)]
struct Cache {
    types: HashMap<i64, (Color32, Option<String>)>,
    statuses: HashMap<i64, (String, Color32)>,
}

/// Alle Terminal-Verknüpfungen: Sitzung → Chips (in der Reihenfolge des Verknüpfens).
fn build_chips(conn: &Connection, lang: Lang) -> HashMap<u64, Vec<Chip>> {
    let mut out: HashMap<u64, Vec<Chip>> = HashMap::new();
    let Ok(mut st) = conn.prepare("SELECT from_ticket_id, to_text FROM links WHERE to_kind = 'terminal' ORDER BY id") else { return out };
    let rows: Vec<(i64, String)> = st.query_map([], |r| Ok((r.get(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default()))).map(|it| it.flatten().collect()).unwrap_or_default();
    let mut cache = Cache::default();
    for (tid, text) in rows {
        let Some(sid) = parse_target(&text) else { continue };
        let list = out.entry(sid).or_default();
        if list.iter().any(|c| c.ticket_id == tid) {
            continue;
        }
        if let Ok(t) = get_ticket(conn, tid) {
            list.push(chip_of(conn, &t, lang, &mut cache).0);
        }
    }
    out
}

/// IDs der Tickets, die mit der Sitzung verknüpft sind.
fn linked_ids(conn: &Connection, session: u64) -> Vec<i64> {
    let Ok(mut st) = conn.prepare("SELECT from_ticket_id FROM links WHERE to_kind = 'terminal' AND to_text = ?1") else { return Vec::new() };
    st.query_map([target_of(session)], |r| r.get(0)).map(|it| it.flatten().collect()).unwrap_or_default()
}

/// Sucht Tickets aller Experten-Projekte nach Schlüssel (`PAN-423`, `pan423`) oder Titel; ohne Eingabe die zuletzt geänderten.
fn search_hits(conn: &Connection, query: &str, lang: Lang, linked: &[i64], limit: usize) -> Vec<Hit> {
    let q = query.trim();
    let Ok(projects) = list_projects(conn) else { return Vec::new() };
    let mut texts: Vec<String> = Vec::new();
    if !q.is_empty() {
        texts.push(q.to_string());
        if let Some(k) = normalize_key_query(q) {
            if !k.eq_ignore_ascii_case(q) {
                texts.push(k);
            }
        }
    }
    let mut found: Vec<crate::planning::tickets::Ticket> = Vec::new();
    for p in projects.iter().filter(|p| !p.system) {
        let filters: Vec<TicketFilter> = if texts.is_empty() {
            vec![TicketFilter { limit: Some(limit as i64), order: TicketOrder::UpdatedDesc, ..Default::default() }]
        } else {
            texts.iter().map(|t| TicketFilter { text: Some(t.clone()), limit: Some(40), order: TicketOrder::NumberDesc, ..Default::default() }).collect()
        };
        for f in filters {
            if let Ok(page) = list_tickets(conn, Some(p.id), &f) {
                for t in page.items {
                    if !found.iter().any(|x| x.id == t.id) {
                        found.push(t);
                    }
                }
            }
        }
    }
    let mut scored: Vec<(u32, crate::planning::tickets::Ticket)> = found.into_iter().map(|t| (relevance(q, &t.key(), &t.title), t)).collect();
    if q.is_empty() {
        scored.sort_by(|a, b| b.1.updated_at.cmp(&a.1.updated_at));
    } else {
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.project_key.cmp(&b.1.project_key)).then_with(|| a.1.number.cmp(&b.1.number)));
    }
    scored.truncate(limit);
    let mut cache = Cache::default();
    scored
        .into_iter()
        .map(|(_, t)| {
            let (chip, status_color) = chip_of(conn, &t, lang, &mut cache);
            Hit { linked: linked.contains(&chip.ticket_id), chip, status_color }
        })
        .collect()
}

/// Legt die Verknüpfung an (System-Typ `terminal`).
pub(super) fn link_ticket_to(conn: &Connection, ticket: i64, session: u64) -> Result<(), TicketError> {
    let lt = link_type_by_key(conn, "terminal")?;
    add_link(conn, ticket, lt.id, Target::Terminal(target_of(session)), &Source::ui())?;
    Ok(())
}

/// Entfernt alle Terminal-Verknüpfungen zwischen Ticket und Sitzung.
fn unlink_ticket(conn: &Connection, ticket: i64, session: u64) -> Result<usize, TicketError> {
    let ids: Vec<i64> = {
        let mut st = conn.prepare("SELECT id FROM links WHERE from_ticket_id = ?1 AND to_kind = 'terminal' AND to_text = ?2")?;
        st.query_map(rusqlite::params![ticket, target_of(session)], |r| r.get(0))?.collect::<Result<_, _>>()?
    };
    for id in &ids {
        remove_link(conn, *id, &Source::ui())?;
    }
    Ok(ids.len())
}

// ------------------------------------------------------------------ Auswahl „laufende Terminals“ (Feldtyp Terminal)

/// Auswahlfeld (Ausklappliste) der aktuellen Sitzungen für Felder vom Typ „Terminal“. `current` = gespeicherter Wert
/// (`session:<id>` oder leer). Gibt `Some(neuer Wert)` zurück, wenn der Nutzer etwas gewählt hat (`Some("")` = Auswahl
/// entfernt), sonst `None`. Beendete Sitzungen stehen grau mit dem Zusatz „beendet“ in der Liste.
#[allow(dead_code)] // API für die Ticket-Ansicht und den Feld-Editor
pub fn pick_terminal(ui: &mut Ui, salt: impl std::hash::Hash, current: &str, lang: Lang) -> Option<String> {
    let sessions = live();
    let label_of = |s: &LiveSession| if s.folder.is_empty() { s.name.clone() } else { format!("{} · {}", s.name, s.folder) };
    let selected = if current.trim().is_empty() {
        lang.t("Kein Terminal", "No terminal").to_string()
    } else {
        match parse_target(current).and_then(|id| sessions.iter().find(|s| s.id == id)) {
            Some(s) => label_of(s),
            None => format!("{} ({})", terminal_label(current, lang), lang.t("beendet", "ended")),
        }
    };
    let mut chosen: Option<String> = None;
    egui::ComboBox::from_id_salt(("pick_terminal", salt)).selected_text(selected).width(260.0).show_ui(ui, |ui| {
        if ui.selectable_label(current.trim().is_empty(), lang.t("Kein Terminal", "No terminal")).clicked() {
            chosen = Some(String::new());
        }
        for s in &sessions {
            let text = if s.running { RichText::new(label_of(s)) } else { RichText::new(format!("{} ({})", label_of(s), lang.t("beendet", "ended"))).weak() };
            if ui.selectable_label(parse_target(current) == Some(s.id), text).clicked() {
                chosen = Some(target_of(s.id));
            }
        }
        if sessions.is_empty() {
            ui.weak(lang.t("Keine Sitzungen", "No sessions"));
        }
    });
    chosen.filter(|c| c != current)
}

// ------------------------------------------------------------------ Chip zeichnen

const CHIP_H: f32 = 18.0;

/// Zeichnet den Chip der Sitzung rechtsbündig bei `right_center` und gibt seine Breite zurück (0 = nichts gezeichnet, weil
/// kein Ticket verknüpft ist oder `max_w` nicht reicht). Klick öffnet das Ticket (bei mehreren eine Liste), Rechtsklick
/// bietet „Verknüpfung entfernen“.
#[allow(clippy::too_many_arguments)]
pub(super) fn chip_row(ui: &mut Ui, salt: Id, session: u64, chips: &[Chip], right_center: Pos2, max_w: f32, lang: Lang, actions: &mut Vec<Action>) -> f32 {
    if chips.is_empty() {
        return 0.0;
    }
    let keys: Vec<&str> = chips.iter().map(|c| c.key.as_str()).collect();
    let col = chips[0].color;
    let g = ui.painter().layout_no_wrap(chip_label(&keys), FontId::proportional(11.5), col);
    let w = 6.0 + 12.0 + 4.0 + g.size().x + 7.0;
    if w > max_w {
        return 0.0;
    }
    let rect = Rect::from_min_size(Pos2::new(right_center.x - w, right_center.y - CHIP_H / 2.0), Vec2::new(w, CHIP_H));
    let resp = ui.interact(rect, salt, Sense::click());
    let hv = style::anim(ui.ctx(), salt.with("h"), resp.hovered(), style::D_FAST);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let p = ui.painter();
    p.rect_filled(rect, CHIP_H / 2.0, col.gamma_multiply(0.16 + 0.14 * hv));
    p.rect_stroke(rect, CHIP_H / 2.0, Stroke::new(1.0_f32, col.gamma_multiply(0.45 + 0.35 * hv)), StrokeKind::Inside);
    planning_ui::type_icon(p, chips[0].icon.as_deref().unwrap_or(""), Pos2::new(rect.min.x + 12.0, rect.center().y), 12.0, col);
    p.galley(Pos2::new(rect.min.x + 22.0, rect.center().y - g.size().y / 2.0), g, col);

    let resp = resp.on_hover_ui(|ui| {
        ui.set_max_width(320.0);
        for c in chips {
            let status = if c.status.is_empty() { String::new() } else { format!(" [{}]", c.status) };
            ui.label(format!("{} · {}{}", c.key, c.title, status));
        }
        ui.add_space(2.0);
        ui.weak(if chips.len() == 1 {
            lang.t("Klick: Ticket öffnen · Rechtsklick: Verknüpfung entfernen", "Click: open ticket · Right-click: remove link")
        } else {
            lang.t("Klick: Liste der Tickets · Rechtsklick: Verknüpfung entfernen", "Click: list of tickets · Right-click: remove link")
        });
    });
    // Liste der Tickets mit „Öffnen“ und „×“ (Menü bei mehreren Tickets, Rechtsklick-Menü immer)
    let list_ui = |ui: &mut Ui, actions: &mut Vec<Action>| {
        for c in chips {
            ui.horizontal(|ui| {
                let status = if c.status.is_empty() { String::new() } else { format!("  [{}]", c.status) };
                if ui.button(format!("{}  {}{}", c.key, c.title, status)).on_hover_text(lang.t("Ticket öffnen", "Open ticket")).clicked() {
                    actions.push(Action::OpenTicket(c.ticket_id));
                    ui.close();
                }
                if ui.small_button("×").on_hover_text(lang.t("Verknüpfung entfernen", "Remove link")).clicked() {
                    actions.push(Action::UnlinkTicket(session, c.ticket_id));
                    ui.close();
                }
            });
        }
    };
    if chips.len() == 1 {
        if resp.clicked() {
            actions.push(Action::OpenTicket(chips[0].ticket_id));
        }
    } else {
        Popup::menu(&resp).id(salt.with("list")).show(|ui| list_ui(ui, actions));
    }
    resp.context_menu(|ui| {
        if chips.len() == 1 {
            if ui.button(lang.t("Ticket öffnen", "Open ticket")).clicked() {
                actions.push(Action::OpenTicket(chips[0].ticket_id));
                ui.close();
            }
            if ui.button(lang.t("Verknüpfung entfernen", "Remove link")).clicked() {
                actions.push(Action::UnlinkTicket(session, chips[0].ticket_id));
                ui.close();
            }
        } else {
            list_ui(ui, actions);
        }
        ui.separator();
        if ui.button(lang.t("Ticket verknüpfen …", "Link ticket …")).clicked() {
            actions.push(Action::LinkTicket(session));
            ui.close();
        }
    });
    w
}

// ------------------------------------------------------------------ Anwendung

impl App {
    /// Pro Bild: Sitzungen veröffentlichen und die Chips ab und zu (oder nach einer Änderung) neu laden.
    pub(super) fn tlinks_frame(&mut self, ctx: &egui::Context) {
        let sessions: Vec<LiveSession> = self
            .entries
            .values()
            .map(|e| {
                let cwd = e.cwd_now();
                let folder = if e.meta.group.is_empty() { Path::new(&cwd).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default() } else { e.meta.group.clone() };
                LiveSession { id: e.meta.id, name: e.display_name(), folder, running: !e.exited() }
            })
            .collect();
        *LIVE.lock().unwrap_or_else(|e| e.into_inner()) = sessions;
        // Sprung aus einem Terminal-Feld der Ticket-Ansicht (Klick auf den Chip).
        if let Some(id) = take_jump() {
            self.open_session_from_ticket(ctx, id);
        }

        if !self.tlinks.dirty && self.tlinks.stamp.is_some_and(|t| t.elapsed() < Duration::from_millis(1000)) {
            return;
        }
        self.tlinks.dirty = false;
        self.tlinks.stamp = Some(Instant::now());
        let lang = self.settings.language;
        let Some(db) = self.db.as_ref() else {
            self.tlinks.enabled = false;
            self.tlinks.chips.clear();
            return;
        };
        // `expert_mode` direkt aus der Datenbank lesen: der Zustand der Todolist wird erst beim ersten Besuch des Reiters geladen.
        self.tlinks.enabled = db.meta("expert_mode").is_some_and(|v| v == "1") && has_expert_project(&db.conn);
        self.tlinks.chips = if self.tlinks.enabled { build_chips(&db.conn, lang) } else { HashMap::new() };
    }

    /// Ticket in der Ticket-Ansicht öffnen, von überall (Chip, Befehls-Palette, Mini-Suche): schaltet auf den Todolist-Reiter der
    /// Titelleiste und meldet das Ticket an; `PlanUi::header` öffnet es im selben Bild (Projektwechsel, Seite oder Panel je nach
    /// Modus, Expertenmodus wird bei Bedarf eingeschaltet). Ohne Datenbank passiert nichts.
    pub(super) fn open_ticket_from_anywhere(&mut self, ticket_id: i64) {
        if self.db.is_none() {
            return;
        }
        self.set_mode(titlebar::Mode::Tasks);
        self.plan.request_open_ticket(ticket_id);
    }

    /// Zur Sitzung springen (aus der Ticket-Ansicht): schaltet auf den Terminal Hub und aktiviert die Sitzung.
    #[allow(dead_code)] // API für die Ticket-Ansicht
    pub(super) fn open_session_from_ticket(&mut self, ctx: &egui::Context, session: u64) {
        if self.entries.contains_key(&session) {
            self.set_mode(titlebar::Mode::Terminals);
            self.apply(ctx, Action::Activate(session));
        }
    }

    /// Mini-Suche öffnen (nur, wenn die Funktion sichtbar ist).
    pub(super) fn tlinks_open_picker(&mut self, session: u64) {
        if self.tlinks.enabled && self.entries.contains_key(&session) {
            self.tlinks.picker = Some(Picker { session, query: String::new(), searched: None, hits: Vec::new(), sel: 0 });
        }
    }

    /// Verknüpfung entfernen (Chip: „×“ oder Rechtsklick).
    /// Die Verknüpfungs-Chips beim nächsten Bild neu laden (nach Änderungen von außen, z. B. dem Schnellmenü).
    pub(super) fn tlinks_invalidate(&mut self) {
        self.tlinks.dirty = true;
        self.tlinks.stamp = None;
    }

    pub(super) fn tlinks_unlink(&mut self, ticket: i64, session: u64) {
        let lang = self.settings.language;
        let Some(db) = self.db.as_ref() else { return };
        match unlink_ticket(&db.conn, ticket, session) {
            Ok(_) => {
                let key = get_ticket(&db.conn, ticket).map(|t| t.key()).unwrap_or_default();
                self.tlinks.dirty = true;
                self.push_toast(format!("{} {}", key, lang.t("ist nicht mehr mit dem Terminal verknüpft.", "is no longer linked to the terminal.")), toasts::ToastKind::Info, Some(key));
            }
            Err(e) => self.push_toast(e.message(lang), toasts::ToastKind::Error, None),
        }
    }

    /// Ticket-Treffer für die Befehls-Palette (höchstens 8, nach Relevanz). Leer ohne Eingabe oder wenn die Funktion aus ist.
    pub(super) fn tlinks_palette_hits(&mut self, query: &str) -> Vec<(String, String, Action)> {
        let q = query.trim();
        if !self.tlinks.enabled || q.is_empty() {
            return Vec::new();
        }
        let fresh = self.tlinks.pal.as_ref().is_some_and(|(pq, at, _)| pq == q && at.elapsed() < Duration::from_millis(1000));
        if !fresh {
            let lang = self.settings.language;
            let hits = self.db.as_ref().map(|db| search_hits(&db.conn, q, lang, &[], 8)).unwrap_or_default();
            self.tlinks.pal = Some((q.to_string(), Instant::now(), hits));
        }
        self.tlinks.pal.as_ref().map(|(_, _, h)| h.as_slice()).unwrap_or(&[]).iter().map(|h| {
            let c = &h.chip;
            (format!("{}  {}", c.key, c.title), c.status.clone(), Action::OpenTicket(c.ticket_id))
        }).collect()
    }

    /// Die Mini-Suche (Fenster oben mittig): Textfeld, Treffer, Pfeiltasten + Enter, Esc schließt.
    pub(super) fn tlinks_picker_ui(&mut self, ctx: &egui::Context) {
        let Some(mut pk) = self.tlinks.picker.take() else { return };
        let lang = self.settings.language;
        if !self.entries.contains_key(&pk.session) || !self.tlinks.enabled {
            return;
        }
        let session_name = self.entries.get(&pk.session).map(|e| e.display_name()).unwrap_or_default();
        let (down, up, enter, esc) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if pk.searched.as_deref() != Some(pk.query.as_str()) {
            let linked = self.db.as_ref().map(|d| linked_ids(&d.conn, pk.session)).unwrap_or_default();
            pk.hits = self.db.as_ref().map(|d| search_hits(&d.conn, &pk.query, lang, &linked, 12)).unwrap_or_default();
            pk.searched = Some(pk.query.clone());
            pk.sel = 0;
        }
        if down && !pk.hits.is_empty() {
            pk.sel = (pk.sel + 1) % pk.hits.len();
        }
        if up && !pk.hits.is_empty() {
            pk.sel = (pk.sel + pk.hits.len() - 1) % pk.hits.len();
        }
        pk.sel = pk.sel.min(pk.hits.len().saturating_sub(1));
        let mut chosen: Option<usize> = if enter && !pk.hits.is_empty() { Some(pk.sel) } else { None };
        let mut close = esc;
        egui::Window::new("tlink_picker")
            .title_bar(false)
            .resizable(false)
            .anchor(Align2::CENTER_TOP, Vec2::new(0.0, 70.0))
            .fixed_size([560.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(lang.t("Ticket verknüpfen mit", "Link ticket to")).strong());
                    ui.label(RichText::new(lang.q(&session_name)).color(style::dim(ui)));
                });
                ui.add_space(4.0);
                let te = ui.add(
                    egui::TextEdit::singleline(&mut pk.query)
                        .desired_width(f32::INFINITY)
                        .hint_text(lang.t("Ticketnummer (z. B. PAN-423) oder Titel …", "Ticket number (e.g. PAN-423) or title …")),
                );
                te.request_focus();
                ui.add_space(4.0);
                let weak = style::dim(ui);
                for (i, h) in pk.hits.iter().enumerate() {
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), Sense::click());
                    if i == pk.sel {
                        ui.painter().rect_filled(rect, 5.0, ui.visuals().selection.bg_fill.gamma_multiply(0.6));
                    } else if resp.hovered() {
                        ui.painter().rect_filled(rect, 5.0, ui.visuals().widgets.hovered.bg_fill);
                    }
                    let p = ui.painter();
                    let c = &h.chip;
                    let dim = if h.linked { 0.45 } else { 1.0 };
                    planning_ui::type_icon(p, c.icon.as_deref().unwrap_or(""), Pos2::new(rect.min.x + 14.0, rect.center().y), 14.0, c.color.gamma_multiply(dim));
                    let key_g = p.layout_no_wrap(c.key.clone(), FontId::proportional(13.0), ui.visuals().strong_text_color().gamma_multiply(dim));
                    let kw = key_g.size().x;
                    p.galley(Pos2::new(rect.min.x + 28.0, rect.center().y - key_g.size().y / 2.0), key_g, Color32::PLACEHOLDER);
                    // rechts: „verknüpft“ bzw. Status
                    let right_text = if h.linked { lang.t("verknüpft", "linked").to_string() } else { c.status.clone() };
                    let right_col = if h.linked { weak } else { h.status_color };
                    let mut rx = rect.max.x - 8.0;
                    if !right_text.is_empty() {
                        let rg = p.layout_no_wrap(right_text, FontId::proportional(11.5), right_col);
                        let rw = rg.size().x;
                        let pill = Rect::from_min_size(Pos2::new(rx - rw - 12.0, rect.center().y - 9.0), Vec2::new(rw + 12.0, 18.0));
                        p.rect_filled(pill, 9.0, right_col.gamma_multiply(0.16));
                        p.galley(Pos2::new(pill.min.x + 6.0, rect.center().y - rg.size().y / 2.0), rg, right_col);
                        rx = pill.min.x - 8.0;
                    }
                    let title_x = rect.min.x + 28.0 + kw + 10.0;
                    let title_g = p.layout_no_wrap(c.title.clone(), FontId::proportional(13.0), ui.visuals().text_color().gamma_multiply(dim));
                    p.with_clip_rect(Rect::from_min_max(Pos2::new(title_x, rect.min.y), Pos2::new(rx.max(title_x + 10.0), rect.max.y))).galley(
                        Pos2::new(title_x, rect.center().y - title_g.size().y / 2.0),
                        title_g,
                        Color32::PLACEHOLDER,
                    );
                    if resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if resp.clicked() {
                        chosen = Some(i);
                    }
                }
                if pk.hits.is_empty() {
                    ui.weak(lang.t("Keine Treffer", "No matches"));
                }
                ui.add_space(4.0);
                ui.weak(lang.t("↑↓ wählen · Enter verknüpfen · Esc schließen", "↑↓ select · Enter link · Esc close"));
            });
        if let Some(i) = chosen {
            let h = pk.hits[i].clone();
            if h.linked {
                self.push_toast(format!("{} {}", h.chip.key, lang.t("ist mit diesem Terminal schon verknüpft.", "is already linked to this terminal.")), toasts::ToastKind::Warning, Some(h.chip.key.clone()));
            } else if let Some(db) = self.db.as_ref() {
                match link_ticket_to(&db.conn, h.chip.ticket_id, pk.session) {
                    Ok(()) => {
                        self.tlinks.dirty = true;
                        self.tlinks.stamp = None;
                        self.push_toast(
                            format!("{} {} {}", h.chip.key, lang.t("mit dem Terminal verknüpft:", "linked to terminal:"), lang.q(&session_name)),
                            toasts::ToastKind::Info,
                            Some(h.chip.key.clone()),
                        );
                        close = true;
                    }
                    Err(e) => self.push_toast(e.message(lang), toasts::ToastKind::Error, None),
                }
            }
        }
        if close {
            self.want_focus = Some(self.focused);
        } else {
            self.tlinks.picker = Some(pk);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::tickets::fixture::Fx;

    #[test]
    fn target_format_round_trips() {
        assert_eq!(target_of(12), "session:12");
        assert_eq!(parse_target("session:12"), Some(12));
        assert_eq!(parse_target(" session:7 "), Some(7));
        for bad in ["", "session:", "session:x", "session:-1", "session:+1", "12", "sess-1", "Session:1", "session:1 2"] {
            assert_eq!(parse_target(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn link_state_follows_the_live_list() {
        let live = vec![
            LiveSession { id: 1, name: "a".into(), folder: String::new(), running: true },
            LiveSession { id: 2, name: "b".into(), folder: String::new(), running: false },
        ];
        assert_eq!(state_in(&live, "session:1"), LinkState::Running(1));
        assert_eq!(state_in(&live, "session:2"), LinkState::Ended, "beendet");
        assert_eq!(state_in(&live, "session:3"), LinkState::Ended, "geschlossen");
        assert_eq!(state_in(&live, "irgendwas"), LinkState::Ended, "kein Sitzungs-Ziel");
        // Neustart der Sitzung (gleiche ID) → wieder aktiv
        let again = vec![LiveSession { running: true, ..live[1].clone() }];
        assert_eq!(state_in(&again, "session:2"), LinkState::Running(2));
    }

    #[test]
    fn normalizes_ticket_keys() {
        for q in ["pan423", "PAN-423", "pan-423", " Pan 423 ", "pan-0423", "PAN423"] {
            assert_eq!(normalize_key_query(q).as_deref(), Some("PAN-423"), "{q:?}");
        }
        assert_eq!(normalize_key_query("pan-4").as_deref(), Some("PAN-4"));
        assert_eq!(normalize_key_query("pan2-17").as_deref(), Some("PAN2-17"));
        for q in ["", "pan", "423", "-5", "fix bug 12x", "9pan-1", "pa n-ü-5"] {
            assert_eq!(normalize_key_query(q), None, "{q:?}");
        }
    }

    #[test]
    fn ranks_exact_key_before_prefix_before_title() {
        let r = |q: &str, k: &str, t: &str| relevance(q, k, t);
        assert!(r("pan423", "PAN-423", "x") > r("pan423", "PAN-4230", "x"));
        assert!(r("pan423", "PAN-4230", "x") > r("pan423", "PAN-5", "mentions pan423 here"));
        assert!(r("login", "PAN-1", "Login page") > r("login", "PAN-2", "Fix the login"));
        assert!(r("login", "PAN-2", "Fix the login") > r("login", "PAN-3", "Other"));
        assert_eq!(r("", "PAN-1", "t"), 1);
    }

    #[test]
    fn chip_label_for_one_and_many() {
        assert_eq!(chip_label(&[]), "");
        assert_eq!(chip_label(&["PAN-1"]), "PAN-1");
        assert_eq!(chip_label(&["PAN-1", "PAN-2"]), "PAN-1 +1");
        assert_eq!(chip_label(&["PAN-1", "PAN-2", "PAN-9"]), "PAN-1 +2");
    }

    #[test]
    fn links_a_session_once_and_survives_removal_of_the_target() {
        let f = Fx::new();
        let a = f.mk(f.task, "Fix login");
        let b = f.mk(f.story, "Dark mode");
        link_ticket_to(&f.c, a.id, 5).unwrap();
        assert_eq!(link_ticket_to(&f.c, a.id, 5).unwrap_err(), TicketError::LinkDuplicate);
        link_ticket_to(&f.c, b.id, 5).unwrap();
        link_ticket_to(&f.c, a.id, 6).unwrap();
        let chips = build_chips(&f.c, Lang::En);
        assert_eq!(chips[&5].iter().map(|c| c.key.as_str()).collect::<Vec<_>>(), vec!["PAN-1", "PAN-2"]);
        assert_eq!(chips[&6].len(), 1);
        assert_eq!(linked_ids(&f.c, 5), vec![a.id, b.id]);
        // Verknüpfung bleibt, auch wenn die Sitzung nicht (mehr) lebt – nur der Zustand ist „beendet“
        assert_eq!(state_in(&[], &target_of(5)), LinkState::Ended);
        assert_eq!(unlink_ticket(&f.c, a.id, 5).unwrap(), 1);
        assert_eq!(unlink_ticket(&f.c, a.id, 5).unwrap(), 0);
        assert_eq!(build_chips(&f.c, Lang::En)[&5].len(), 1);
        assert_eq!(build_chips(&f.c, Lang::En)[&6].len(), 1);
    }

    #[test]
    fn searches_by_key_and_title_over_expert_projects_only() {
        let f = Fx::new();
        let a = f.mk(f.task, "Fix login");
        f.mk(f.story, "Dark mode");
        f.mk(f.task, "Login page redesign");
        let keys = |q: &str| search_hits(&f.c, q, Lang::En, &[], 8).into_iter().map(|h| h.chip.key).collect::<Vec<_>>();
        assert_eq!(keys("pan1"), vec!["PAN-1"]);
        assert_eq!(keys("PAN-2"), vec!["PAN-2"]);
        assert_eq!(keys("pan-3"), vec!["PAN-3"]);
        assert_eq!(keys("login"), vec!["PAN-3", "PAN-1"], "Titel-Anfang vor Titel-Teil");
        assert_eq!(keys("dark"), vec!["PAN-2"]);
        assert!(keys("nothing at all").is_empty());
        assert_eq!(keys("").len(), 3, "ohne Eingabe: zuletzt geänderte");
        let hits = search_hits(&f.c, "pan1", Lang::En, &[a.id], 8);
        assert!(hits[0].linked);
        // Hinweis: das Todolist-Projekt (System) kommt nicht vor
        assert!(has_expert_project(&f.c));
        assert!(!has_expert_project(&crate::db::open_memory()));
    }
}
