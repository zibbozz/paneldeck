//! Reiter „Dashboards“: Dashboard-Wechsler (neu, duplizieren, umbenennen, löschen, Standard, Export/Import), Raster aus Kacheln und
//! der Bearbeiten-Modus (Kachel hinzufügen, entfernen, konfigurieren, per Ziehen oder Pfeilen verschieben, per Ecke in der Größe ändern).
//!
//! Einhängepunkte:
//! * `PlanUi::dash_tab` wird vom Reiter „Dashboards“ aufgerufen (`mod.rs`, volle Breite und Höhe, ohne die gemeinsame Rollfläche). Der
//!   Reiter gibt es in jedem Projekt (dann Projekt- und globale Dashboards) und im System-Projekt (nur die globalen Dashboards).
//! * `PlanUi::dash_dialogs` (in `dash_dialogs.rs`) zeichnet die Dialoge.
//! * Klicks auf Zeilen öffnen das Ticket (`PlanUi::open_ticket`), Klicks auf Diagramm-Segmente und „Filter öffnen“ die Suche mit der
//!   Abfrage (`PlanUi::open_search_query`), beides in `ticket_nav.rs`; „Zurück“ der Ticket-Ansicht führt wieder zum Dashboard.
//!
//! **Daten:** Jede Kachel liest über `dash_data.rs` und wird **zwischengespeichert** (`State::cache`): neu geladen wird beim Öffnen des
//! Reiters, bei geänderten Einstellungen der Kachel, nach Änderungen von außen (`PlanUi::invalidate`) und alle [`TTL`] Sekunden – nie
//! jedes Bild, und höchstens [`MAX_LOADS`] Kacheln pro Bild, damit große Projekte die Oberfläche nicht ruckeln lassen.
//! Die reine Logik (Raster, Zwischenspeicher-Regeln, Gruppen, Skalierung) steht in `dash_logic.rs`.

use super::dash_data::{Job, Loader, TileData, Worker};
use super::dash_dialogs::Dlg;
use super::dash_logic::{self as logic, CacheMeta, DragMode, Grid, KindSel, TileDraft, SPARE_ROWS};
use super::dash_tiles::{draw_body, BodyCx, Event};
use super::widgets;
use super::{Cx, PlanUi, Tab};
use crate::planning::projects::Project;
use crate::planning::views::dashboards::{clear_default, duplicate_dashboard, list_available, list_dashboards, set_default, startup_dashboard, update_dashboard, Dashboard, Layout, Tile, UpdateDashboard};
use crate::state::Lang;
use crate::style::{self, Icon, S2};
use eframe::egui::{self, Align, Color32, Id, Layout as UiLayout, Pos2, Rect, RichText, Sense, Shape, Stroke, StrokeKind, Ui, UiBuilder, Vec2};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// So lange gelten geladene Kacheldaten, bevor sie neu gelesen werden.
pub const TTL: Duration = Duration::from_secs(8);
/// Höchstzahl der Kacheln, die in einem Bild neu geladen werden, und das Zeitbudget dafür (mindestens eine wird immer geladen).
pub const MAX_LOADS: usize = 2;
/// Höchstzahl der Aufträge, die gleichzeitig beim Hintergrund-Leser warten.
const MAX_INFLIGHT: usize = 6;
const LOAD_BUDGET: Duration = Duration::from_millis(30);
const HEADER_H: f32 = 28.0;

struct Notice {
    text: String,
    error: bool,
    at: Instant,
}

struct Cached {
    meta: CacheMeta,
    loaded: Instant,
    data: TileData,
}

/// Eine laufende Ziehaktion (Verschieben oder Größe ändern).
pub(super) struct Drag {
    pub id: u32,
    pub mode: DragMode,
    pub grab: Vec2,
}

pub(super) struct State {
    /// Projekt, für das die Liste geladen ist (0 = noch keins).
    pub(super) project: i64,
    pub(super) use_sprints: bool,
    pub(super) list: Vec<Dashboard>,
    /// Gewähltes Dashboard (0 = keins).
    pub(super) cur: i64,
    /// Je Projekt gewähltes Dashboard (gilt, solange Paneldeck läuft; beim Start gilt das Standard-Dashboard).
    pub(super) chosen: HashMap<i64, i64>,
    /// Liste neu laden.
    pub(super) stale: bool,
    /// Stand der Daten: eine Änderung entwertet alle Kachel-Zwischenspeicher.
    pub(super) generation: u64,
    pub(super) edit: bool,
    cache: HashMap<u32, Cached>,
    /// Der Hintergrund-Leser (nur mit einer Datenbank-Datei) und die Aufträge, die bei ihm warten: Kachel → (Schlüssel, Stand).
    worker: Option<Worker>,
    worker_failed: bool,
    inflight: HashMap<u32, (u64, u64)>,
    drag: Option<Drag>,
    pub(super) dlg: Option<Dlg>,
    notice: Option<Notice>,
    /// Bild-Nummer des letzten Zeichnens (um das Öffnen des Reiters zu erkennen).
    last_frame: u64,
}

impl Default for State {
    fn default() -> Self {
        State { project: 0, use_sprints: false, list: Vec::new(), cur: 0, chosen: HashMap::new(), stale: true, generation: 1, edit: false, cache: HashMap::new(), worker: None, worker_failed: false, inflight: HashMap::new(), drag: None, dlg: None, notice: None, last_frame: 0 }
    }
}

impl State {
    pub(super) fn current(&self) -> Option<&Dashboard> {
        self.list.iter().find(|d| d.id == self.cur)
    }

    pub(super) fn notify(&mut self, text: String, error: bool) {
        self.notice = Some(Notice { text, error, at: Instant::now() });
    }

    /// Alle gespeicherten Kacheldaten gelten als veraltet und werden neu gelesen.
    pub(super) fn invalidate(&mut self) {
        self.generation += 1;
    }

    /// Anderes Dashboard wählen (leert den Zwischenspeicher und verlässt den Bearbeiten-Modus).
    pub(super) fn select(&mut self, id: i64) {
        if self.cur != id {
            self.cur = id;
            self.cache.clear();
            self.inflight.clear();
            self.drag = None;
            self.edit = false;
        }
        self.chosen.insert(self.project, id);
    }

    /// Welche Kacheln müssen neu geladen werden? (Kachel, hat schon Daten.)
    fn pending(&self, layout: &Layout, dash_project: Option<i64>, lang: Lang, now: Instant) -> Vec<(u32, bool)> {
        layout
            .tiles
            .iter()
            .filter_map(|t| {
                let key = logic::tile_key(&t.kind, dash_project, lang);
                let have = self.cache.get(&t.id);
                let age = have.map(|c| now.saturating_duration_since(c.loaded)).unwrap_or_default();
                logic::needs_reload(have.map(|c| c.meta), key, self.generation, age, TTL).then_some((t.id, have.is_some()))
            })
            .collect()
    }
}

/// Aktionen im Kopf des Reiters (werden nach dem Zeichnen ausgeführt).
enum HeaderAct {
    Pick(i64),
    ToggleDefault,
    New,
    Rename,
    Duplicate,
    Export,
    Import,
    Delete,
    Edit(bool),
    AddTile,
    Tidy,
}

/// Aktionen der Kacheln (werden nach dem Zeichnen ausgeführt).
enum TileAct {
    StartMove { id: u32, grab: Vec2 },
    StartResize { id: u32, grab: Vec2 },
    Nudge { id: u32, dx: i32, dy: i32 },
    Config(u32),
    Remove(u32),
}

impl PlanUi {
    /// Die Daten haben sich außerhalb des Reiters geändert: Liste und alle Kacheln beim nächsten Bild neu lesen.
    pub(super) fn dash_invalidate(&mut self) {
        self.dash.stale = true;
        self.dash.invalidate();
    }

    // ------------------------------------------------------------------ Laden

    fn dash_load_list(cx: &Cx, scope_project: Option<i64>) -> Vec<Dashboard> {
        match scope_project {
            Some(p) => list_available(cx.conn, p).unwrap_or_default(),
            None => list_dashboards(cx.conn, None).unwrap_or_default(),
        }
    }

    /// Liste der Dashboards, Auswahl und das Standard-Dashboard bereitstellen (beim ersten Öffnen eines Bereichs entsteht „Dashboard“
    /// mit einer großen Such-Kachel – nur einmal, auch wenn später alle gelöscht werden).
    fn dash_sync(&mut self, cx: &Cx, proj: &Project) {
        let lang = cx.lang;
        if self.dash.project != proj.id {
            let chosen = std::mem::take(&mut self.dash.chosen);
            let worker = self.dash.worker.take();
            self.dash = State { project: proj.id, chosen, worker, ..State::default() };
        }
        self.dash.use_sprints = proj.use_sprints;
        if !self.dash.stale {
            return;
        }
        self.dash.stale = false;
        let scope_project = (!proj.system).then_some(proj.id);
        let mut list = Self::dash_load_list(cx, scope_project);
        let seeded = format!("dash_seeded_{}", scope_project.map(|p| p.to_string()).unwrap_or_else(|| "global".to_string()));
        if list.is_empty() && cx.db.meta(&seeded).is_none() {
            cx.db.set_meta(&seeded, "1");
            if crate::planning::views::dashboards::ensure_default_dashboard(cx.conn, scope_project, lang).is_ok() {
                list = Self::dash_load_list(cx, scope_project);
            }
        }
        if !list.iter().any(|d| d.id == self.dash.cur) {
            let has = |id: &i64| list.iter().any(|d| d.id == *id);
            let chosen = self.dash.chosen.get(&proj.id).copied().filter(has);
            let startup = startup_dashboard(cx.conn, scope_project).ok().flatten().map(|d| d.id).filter(has);
            self.dash.cur = chosen.or(startup).or(list.first().map(|d| d.id)).unwrap_or(0);
            self.dash.cache.clear();
            self.dash.inflight.clear();
            self.dash.drag = None;
        }
        self.dash.list = list;
    }

    /// Sorgt dafür, dass die Kacheln aktuell werden: Fertige Ergebnisse des Hintergrund-Lesers einsammeln und für Kacheln, deren Daten
    /// fehlen oder veraltet sind, Aufträge abgeben. Mit einer Datenbank-Datei liest ein Hintergrund-Faden (die Oberfläche wartet nie);
    /// ohne (Datenbank im Speicher, Tests) wird gleich hier geladen, höchstens [`MAX_LOADS`] Kacheln pro Bild.
    /// `true`, wenn im nächsten Bild noch Arbeit wartet.
    fn dash_load_tiles(&mut self, ctx: &egui::Context, cx: &Cx, cur: &Dashboard) -> bool {
        let lang = cx.lang;
        let today = chrono::Local::now().date_naive();
        let use_worker = !cx.db.path.as_os_str().is_empty() && !self.dash.worker_failed;
        if use_worker && self.dash.worker.is_none() {
            let wake = ctx.clone();
            match Worker::spawn(cx.db.path.clone(), move || wake.request_repaint()) {
                Some(w) => self.dash.worker = Some(w),
                None => self.dash.worker_failed = true,
            }
        }
        if let Some(w) = &self.dash.worker {
            for d in w.poll() {
                if self.dash.inflight.get(&d.tile) == Some(&(d.key, d.generation)) {
                    self.dash.inflight.remove(&d.tile);
                }
                if d.dash_id == cur.id && cur.layout.tile(d.tile).is_some() {
                    self.dash.cache.insert(d.tile, Cached { meta: CacheMeta { key: d.key, generation: d.generation }, loaded: Instant::now(), data: d.data });
                }
            }
        }
        let pending = self.dash.pending(&cur.layout, cur.project_id, lang, Instant::now());
        let generation = self.dash.generation;
        let key_of = |t: &Tile| logic::tile_key(&t.kind, cur.project_id, lang);
        let mut more = false;
        if self.dash.worker.is_some() {
            let todo: Vec<(u32, bool)> = pending.iter().copied().filter(|(id, _)| cur.layout.tile(*id).is_some_and(|t| self.dash.inflight.get(id) != Some(&(key_of(t), generation)))).collect();
            let room = MAX_INFLIGHT.saturating_sub(self.dash.inflight.len());
            for id in logic::pick_loads(&todo, room) {
                let Some(t) = cur.layout.tile(id) else { continue };
                let key = key_of(t);
                let job = Job { tile: id, key, generation, dash_id: cur.id, dash_project: cur.project_id, kind: t.kind.clone(), lang, today };
                if self.dash.worker.as_ref().is_some_and(|w| w.submit(job)) {
                    self.dash.inflight.insert(id, (key, generation));
                } else {
                    // der Faden ist weg: ab jetzt im Vordergrund laden
                    self.dash.worker = None;
                    self.dash.worker_failed = true;
                    self.dash.inflight.clear();
                    more = true;
                    break;
                }
            }
        } else {
            let picks = logic::pick_loads(&pending, MAX_LOADS);
            let loader = Loader { conn: cx.conn, lang, today, dash_project: cur.project_id };
            let started = Instant::now();
            let mut done = 0;
            for id in &picks {
                if done > 0 && started.elapsed() > LOAD_BUDGET {
                    break;
                }
                let Some(t) = cur.layout.tile(*id) else { continue };
                let data = loader.load(&t.kind);
                self.dash.cache.insert(*id, Cached { meta: CacheMeta { key: key_of(t), generation }, loaded: Instant::now(), data });
                done += 1;
            }
            more = pending.len() > done;
        }
        self.dash.cache.retain(|id, _| cur.layout.tile(*id).is_some());
        more
    }

    // ------------------------------------------------------------------ Speichern

    /// Speichert das Layout des Dashboards; bei einem Fehler die (zweisprachige) Meldung.
    pub(super) fn dash_try_save(&mut self, cx: &Cx, id: i64, mut layout: Layout) -> Result<(), String> {
        layout.normalize();
        match update_dashboard(cx.conn, id, &UpdateDashboard { name: None, layout: Some(layout.clone()) }) {
            Ok(()) => {
                if let Some(d) = self.dash.list.iter_mut().find(|d| d.id == id) {
                    d.layout = layout;
                }
                Ok(())
            }
            Err(e) => {
                self.dash.stale = true;
                Err(e.message(cx.lang))
            }
        }
    }

    fn dash_save(&mut self, cx: &Cx, id: i64, layout: Layout) {
        if let Err(m) = self.dash_try_save(cx, id, layout) {
            self.dash.notify(m, true);
        }
    }

    /// Wählt ein Dashboard (die Liste ist danach neu zu lesen, falls es neu ist).
    pub(super) fn dash_select(&mut self, id: i64) {
        self.dash.select(id);
        self.dash.stale = true;
    }

    // ------------------------------------------------------------------ Reiter

    /// Der Reiter „Dashboards“ (nimmt die ganze Breite und Höhe unter den Reitern).
    pub(super) fn dash_tab(&mut self, ui: &mut Ui, cx: &Cx, proj: &Project) {
        let lang = cx.lang;
        let ctx = ui.ctx().clone();
        self.dash_sync(cx, proj);
        // Reiter gerade geöffnet (das letzte Bild hat ihn nicht gezeichnet): alles neu lesen
        let frame = ctx.cumulative_frame_nr();
        if frame > self.dash.last_frame + 1 {
            self.dash.invalidate();
        }
        self.dash.last_frame = frame;
        self.dash_header(ui, cx, proj);
        ui.add_space(S2);
        let Some(cur) = self.dash.current().cloned() else {
            ui.add_space(S2);
            widgets::hint(ui, lang.t("Noch kein Dashboard. Lege mit „Neues Dashboard“ eines an oder importiere eines.", "No dashboard yet. Create one with “New dashboard” or import one."));
            self.dash_notice(&ctx);
            return;
        };
        let more = self.dash_load_tiles(&ctx, cx, &cur);
        self.dash_canvas(ui, cx, &cur);
        self.dash_notice(&ctx);
        if more || self.dash.drag.is_some() {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(Duration::from_secs(2));
        }
    }

    fn dash_header(&mut self, ui: &mut Ui, cx: &Cx, proj: &Project) {
        let lang = cx.lang;
        let cur = self.dash.current().map(|d| (d.id, d.name.clone(), d.is_default));
        let edit = self.dash.edit;
        let mut act: Option<HeaderAct> = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if let Some((cur_id, name, is_default)) = &cur {
                egui::ComboBox::from_id_salt("dash_pick").selected_text(name.as_str()).width(230.0).show_ui(ui, |ui| {
                    let mine: Vec<&Dashboard> = self.dash.list.iter().filter(|d| d.project_id.is_some()).collect();
                    let global: Vec<&Dashboard> = self.dash.list.iter().filter(|d| d.project_id.is_none()).collect();
                    let both = !mine.is_empty() && !global.is_empty();
                    for (title, group) in [(lang.t("Dieses Projekt", "This project"), mine), (lang.t("Alle Projekte (global)", "All projects (global)"), global)] {
                        if both {
                            ui.label(RichText::new(title).small().color(style::dim(ui)));
                        }
                        for d in group {
                            let label = if d.is_default { format!("{}  ({})", d.name, lang.t("Standard", "default")) } else { d.name.clone() };
                            if ui.selectable_label(d.id == *cur_id, label).clicked() {
                                act = Some(HeaderAct::Pick(d.id));
                            }
                        }
                    }
                });
                if star_button(ui, *is_default).on_hover_text(lang.t("Als Standard-Dashboard festlegen (wird beim Start zuerst gezeigt)", "Use as the default dashboard (shown first at startup)")).clicked() {
                    act = Some(HeaderAct::ToggleDefault);
                }
                let gear = style::icon_btn(ui, Icon::Gear, lang.t("Dashboard-Menü", "Dashboard menu"), false);
                egui::Popup::from_toggle_button_response(&gear).close_behavior(egui::PopupCloseBehavior::CloseOnClick).show(|ui| {
                    ui.set_min_width(190.0);
                    if ui.button(lang.t("Umbenennen …", "Rename …")).clicked() {
                        act = Some(HeaderAct::Rename);
                    }
                    if ui.button(lang.t("Duplizieren", "Duplicate")).clicked() {
                        act = Some(HeaderAct::Duplicate);
                    }
                    ui.separator();
                    if ui.button(lang.t("Exportieren (JSON) …", "Export (JSON) …")).clicked() {
                        act = Some(HeaderAct::Export);
                    }
                    if ui.button(lang.t("Importieren (JSON) …", "Import (JSON) …")).clicked() {
                        act = Some(HeaderAct::Import);
                    }
                    ui.separator();
                    if widgets::small_danger(ui, lang.t("Löschen …", "Delete …")).clicked() {
                        act = Some(HeaderAct::Delete);
                    }
                });
            } else if ui.button(lang.t("Importieren (JSON) …", "Import (JSON) …")).clicked() {
                act = Some(HeaderAct::Import);
            }
            if ui.button(format!("+ {}", lang.t("Neues Dashboard", "New dashboard"))).clicked() {
                act = Some(HeaderAct::New);
            }
            if cur.is_some() {
                ui.with_layout(UiLayout::right_to_left(Align::Center), |ui| {
                    if edit {
                        if widgets::primary(ui, lang.t("Fertig", "Done")).clicked() {
                            act = Some(HeaderAct::Edit(false));
                        }
                        if ui.button(lang.t("Aufräumen", "Tidy up")).on_hover_text(lang.t("Schließt Lücken zwischen den Kacheln", "Closes the gaps between the tiles")).clicked() {
                            act = Some(HeaderAct::Tidy);
                        }
                        if ui.button(format!("+ {}", lang.t("Kachel", "Tile"))).on_hover_text(lang.t("Kachel hinzufügen …", "Add a tile …")).clicked() {
                            act = Some(HeaderAct::AddTile);
                        }
                    } else if widgets::secondary(ui, lang.t("Bearbeiten", "Edit")).on_hover_text(lang.t("Kacheln hinzufügen, verschieben, ändern, entfernen", "Add, move, change and remove tiles")).clicked() {
                        act = Some(HeaderAct::Edit(true));
                    }
                });
            }
        });
        let Some(act) = act else { return };
        let Some((cur_id, cur_name, is_default)) = cur else {
            match act {
                HeaderAct::New => self.open_new_dashboard(proj, lang),
                HeaderAct::Import => self.dash_start_import(cx, proj),
                _ => {}
            }
            return;
        };
        match act {
            HeaderAct::Pick(id) => self.dash_select(id),
            HeaderAct::ToggleDefault => {
                let project_id = self.dash.current().and_then(|d| d.project_id);
                let r = if is_default { clear_default(cx.conn, project_id) } else { set_default(cx.conn, cur_id) };
                match r {
                    Ok(()) => {
                        self.dash.stale = true;
                        self.dash.notify(if is_default { lang.t("Kein Standard-Dashboard mehr.", "No default dashboard any more.").to_string() } else { lang.t("Als Standard-Dashboard festgelegt.", "Set as the default dashboard.").to_string() }, false);
                    }
                    Err(e) => self.dash.notify(e.message(lang), true),
                }
            }
            HeaderAct::New => self.open_new_dashboard(proj, lang),
            HeaderAct::Rename => self.open_rename_dashboard(cur_id, cur_name),
            HeaderAct::Duplicate => match duplicate_dashboard(cx.conn, cur_id, lang) {
                Ok(id) => {
                    self.dash_select(id);
                    self.dash.notify(lang.t("Dashboard dupliziert.", "Dashboard duplicated.").to_string(), false);
                }
                Err(e) => self.dash.notify(e.message(lang), true),
            },
            HeaderAct::Export => self.dash_export(cx, cur_id),
            HeaderAct::Import => self.dash_start_import(cx, proj),
            HeaderAct::Delete => self.open_delete_dashboard(cur_id, cur_name),
            HeaderAct::Edit(on) => {
                self.dash.edit = on;
                self.dash.drag = None;
            }
            HeaderAct::AddTile => self.open_tile_dialog(cx, None),
            HeaderAct::Tidy => {
                if let Some(d) = self.dash.current().cloned() {
                    let mut l = d.layout.clone();
                    l.pack();
                    if l != d.layout {
                        self.dash_save(cx, d.id, l);
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------ Raster

    fn dash_canvas(&mut self, ui: &mut Ui, cx: &Cx, cur: &Dashboard) {
        let lang = cx.lang;
        let ctx = ui.ctx().clone();
        let edit = self.dash.edit;
        let pointer = ctx.input(|i| i.pointer.latest_pos());
        let mut events: Vec<Event> = Vec::new();
        let mut acts: Vec<TileAct> = Vec::new();
        let mut shown: Option<Layout> = None;
        egui::ScrollArea::vertical().id_salt(("dash_scroll", cur.id)).auto_shrink([false, false]).show(ui, |ui| {
            let width = ui.available_width();
            let origin = ui.available_rect_before_wrap().min;
            let grid = Grid::new(origin.x, origin.y, width);
            let preview = match (&self.dash.drag, pointer) {
                (Some(d), Some(p)) => logic::preview_layout(&cur.layout, d.id, d.mode, d.grab, &grid, p),
                _ => cur.layout.clone(),
            };
            let rows = logic::layout_rows(&preview) + if edit { SPARE_ROWS } else { 0 };
            let (canvas, _) = ui.allocate_exact_size(Vec2::new(width, Grid::height(rows.max(1))), Sense::hover());
            if edit {
                let dot = ui.visuals().widgets.noninteractive.bg_stroke.color.gamma_multiply(0.7);
                for r in 0..rows {
                    for c in 0..crate::planning::views::dashboards::GRID_COLUMNS {
                        let p = Pos2::new(grid.left + c as f32 * grid.pitch_x(), grid.top + r as f32 * grid.pitch_y());
                        ui.painter().circle_filled(p, 1.2, dot);
                    }
                }
            }
            let active = self.dash.drag.as_ref().map(|d| d.id);
            let mut order: Vec<&Tile> = preview.tiles.iter().collect();
            order.sort_by_key(|t| Some(t.id) == active);
            for t in order {
                let rect = grid.tile_rect(t);
                if !rect.intersects(ui.clip_rect().expand(40.0)) && Some(t.id) != active {
                    continue;
                }
                let data = self.dash.cache.get(&t.id).map(|c| &c.data);
                tile_ui(ui, lang, t, rect, data, edit, Some(t.id) == active, &mut acts, &mut events);
            }
            // Beim Ziehen am oberen/unteren Rand der sichtbaren Fläche rollen
            if let (Some(_), Some(p)) = (&self.dash.drag, pointer) {
                let clip = ui.clip_rect();
                if p.y > clip.bottom() - 36.0 {
                    ui.scroll_with_delta(Vec2::new(0.0, -14.0));
                } else if p.y < clip.top() + 36.0 {
                    ui.scroll_with_delta(Vec2::new(0.0, 14.0));
                }
            }
            let _ = canvas;
            shown = Some(preview);
        });
        // Ziehen beenden: Loslassen übernimmt, Esc verwirft
        if self.dash.drag.is_some() {
            let drag_mode = self.dash.drag.as_ref().map(|d| d.mode);
            ctx.set_cursor_icon(if drag_mode == Some(DragMode::Resize) { egui::CursorIcon::ResizeNwSe } else { egui::CursorIcon::Grabbing });
            let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
            let down = ctx.input(|i| i.pointer.primary_down());
            if esc {
                self.dash.drag = None;
            } else if !down {
                self.dash.drag = None;
                if let Some(l) = shown {
                    if l != cur.layout {
                        self.dash_save(cx, cur.id, l);
                    }
                }
            }
        }
        self.dash_tile_acts(cx, cur.id, acts);
        for e in events {
            match e {
                Event::OpenTicket(id) => {
                    self.open_ticket(cx, id);
                }
                Event::OpenFilter { jql } => self.open_search_query(cx, &jql, cur.project_id),
                Event::JumpProject(id) => {
                    self.select(cx, id);
                    self.tab = Tab::Overview;
                }
            }
        }
    }

    fn dash_tile_acts(&mut self, cx: &Cx, dash_id: i64, acts: Vec<TileAct>) {
        for a in acts {
            let Some(layout) = self.dash.current().filter(|d| d.id == dash_id).map(|d| d.layout.clone()) else { return };
            match a {
                TileAct::StartMove { id, grab } => self.dash.drag = Some(Drag { id, mode: DragMode::Move, grab }),
                TileAct::StartResize { id, grab } => self.dash.drag = Some(Drag { id, mode: DragMode::Resize, grab }),
                TileAct::Nudge { id, dx, dy } => {
                    let mut l = layout;
                    if logic::nudge(&mut l, id, dx, dy) {
                        self.dash_save(cx, dash_id, l);
                    }
                }
                TileAct::Config(id) => self.open_tile_dialog(cx, Some(id)),
                TileAct::Remove(id) => {
                    let mut l = layout;
                    if l.remove_tile(id) {
                        self.dash_save(cx, dash_id, l);
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------ Meldung

    fn dash_notice(&mut self, ctx: &egui::Context) {
        let Some(n) = &self.dash.notice else { return };
        if n.at.elapsed() > Duration::from_secs(6) {
            self.dash.notice = None;
            return;
        }
        let (text, error) = (n.text.clone(), n.error);
        egui::Area::new(Id::new("dash_toast")).anchor(egui::Align2::CENTER_BOTTOM, Vec2::new(0.0, -18.0)).order(egui::Order::Tooltip).interactable(false).show(ctx, |ui| {
            let mut f = egui::Frame::popup(ui.style()).corner_radius(egui::CornerRadius::same(10));
            if error {
                f = f.stroke(Stroke::new(1.2_f32, widgets::DANGER));
            }
            f.show(ui, |ui| {
                ui.set_max_width(520.0);
                ui.label(text);
            });
        });
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    /// Dialog „Kachel hinzufügen/ändern“ öffnen (`Some(id)` = die Kachel ändern).
    pub(super) fn open_tile_dialog(&mut self, cx: &Cx, tile: Option<u32>) {
        let Some(d) = self.dash.current().cloned() else { return };
        let draft = match tile {
            Some(id) => match d.layout.tile(id) {
                Some(t) => TileDraft::from_tile(t),
                None => return,
            },
            None => TileDraft::new(KindSel::Search),
        };
        self.dash.dlg = Some(Dlg::tile(cx, &d, tile, draft));
    }
}

// ------------------------------------------------------------------ Zeichnen einer Kachel

/// Kleiner Stern-Knopf („Standard“).
fn star_button(ui: &mut Ui, on: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(28.0, 28.0), Sense::click());
    let gold = Color32::from_rgb(0xe5, 0xc0, 0x4b);
    if on {
        ui.painter().rect_filled(rect, style::RADIUS as f32, gold.gamma_multiply(0.2));
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, style::RADIUS as f32, ui.visuals().widgets.hovered.bg_fill);
    }
    widgets::type_icon(ui.painter(), "star", rect.center(), 17.0, if on { gold } else { ui.visuals().weak_text_color() });
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// Kleiner Pfeil-Knopf (verschiebt die Kachel um eine Zelle).
fn arrow_button(ui: &mut Ui, dx: i32, dy: i32, tip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(20.0, 22.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 5.0, ui.visuals().widgets.hovered.bg_fill);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let c = rect.center();
    let col = if resp.hovered() { ui.visuals().strong_text_color() } else { ui.visuals().weak_text_color() };
    let (d, n) = (Vec2::new(dx as f32, dy as f32), Vec2::new(-dy as f32, dx as f32));
    let tri = vec![c + d * 4.5, c - d * 3.0 + n * 4.0, c - d * 3.0 - n * 4.0];
    ui.painter().add(Shape::convex_polygon(tri, col, Stroke::NONE));
    resp.on_hover_text(tip)
}

#[allow(clippy::too_many_arguments)]
fn tile_ui(ui: &mut Ui, lang: Lang, t: &Tile, rect: Rect, data: Option<&TileData>, edit: bool, active: bool, acts: &mut Vec<TileAct>, events: &mut Vec<Event>) {
    let id = Id::new(("dash_tile", t.id));
    let accent = style::accent();
    let (fill, line, text) = (ui.visuals().faint_bg_color, ui.visuals().widgets.noninteractive.bg_stroke.color, ui.visuals().strong_text_color());
    let jql = logic::tile_jql(&t.kind);
    let painter = ui.painter().clone();
    painter.rect_filled(rect, 8.0, if active { ui.visuals().window_fill } else { fill });
    let stroke = if active {
        Stroke::new(2.0_f32, accent)
    } else if edit {
        Stroke::new(1.0_f32, accent.gamma_multiply(0.45))
    } else {
        Stroke::new(1.0_f32, line)
    };
    painter.rect_stroke(rect, 8.0, stroke, StrokeKind::Inside);
    let header = Rect::from_min_size(rect.min, Vec2::new(rect.width(), HEADER_H));
    let ctrl_w = if edit { 4.0 * 20.0 + 2.0 * 24.0 + 10.0 } else if jql.is_some() { 30.0 } else { 0.0 };
    let title_rect = Rect::from_min_max(Pos2::new(header.left() + 10.0, header.top()), Pos2::new((header.right() - ctrl_w - 4.0).max(header.left() + 12.0), header.bottom()));
    painter.with_clip_rect(title_rect).text(Pos2::new(title_rect.left(), header.center().y), egui::Align2::LEFT_CENTER, logic::tile_title(&t.kind, lang), egui::FontId::proportional(13.5), text);
    if edit {
        // Kopf anfassen = verschieben (zuerst eintragen, damit die Knöpfe darüber liegen)
        let drag_rect = Rect::from_min_max(header.min, Pos2::new(header.right() - ctrl_w, header.bottom()));
        let r = ui.interact(drag_rect, id.with("move"), Sense::drag());
        if r.hovered() && !active {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if r.drag_started() {
            let p = ui.ctx().input(|i| i.pointer.press_origin()).or(r.interact_pointer_pos()).unwrap_or(rect.min);
            acts.push(TileAct::StartMove { id: t.id, grab: p - rect.min });
        }
    }
    if ctrl_w > 0.0 {
        let ctrl = Rect::from_min_max(Pos2::new(header.right() - ctrl_w, header.top()), Pos2::new(header.right() - 4.0, header.bottom()));
        ui.scope_builder(UiBuilder::new().id_salt(id.with("ctrl")).max_rect(ctrl).layout(UiLayout::right_to_left(Align::Center)), |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            if edit {
                if style::icon_btn_small(ui, Icon::Close, lang.t("Kachel entfernen", "Remove tile")).clicked() {
                    acts.push(TileAct::Remove(t.id));
                }
                ui.add_space(2.0);
                if style::icon_btn_small(ui, Icon::Gear, lang.t("Kachel einstellen …", "Tile settings …")).clicked() {
                    acts.push(TileAct::Config(t.id));
                }
                ui.add_space(4.0);
                for (dx, dy, tip) in [(0, 1, lang.t("Nach unten", "Move down")), (0, -1, lang.t("Nach oben", "Move up")), (1, 0, lang.t("Nach rechts", "Move right")), (-1, 0, lang.t("Nach links", "Move left"))] {
                    if arrow_button(ui, dx, dy, tip).clicked() {
                        acts.push(TileAct::Nudge { id: t.id, dx, dy });
                    }
                }
            } else if let Some(q) = &jql {
                if style::icon_btn_small(ui, Icon::Search, lang.t("Als Filter öffnen", "Open as filter")).clicked() {
                    events.push(Event::OpenFilter { jql: q.clone() });
                }
            }
        });
    }
    // Inhalt
    let body = Rect::from_min_max(Pos2::new(rect.left() + 10.0, header.bottom() + 2.0), Pos2::new(rect.right() - 10.0, rect.bottom() - 8.0));
    if body.width() > 20.0 && body.height() > 14.0 {
        ui.scope_builder(UiBuilder::new().id_salt(id.with("body")).max_rect(body).layout(UiLayout::top_down(Align::Min)), |ui| {
            ui.set_clip_rect(body.intersect(ui.clip_rect()));
            let cx = BodyCx { lang, tile_id: t.id, kind: &t.kind, interactive: !edit };
            draw_body(ui, &cx, data, events);
        });
    }
    if edit {
        let h = Rect::from_min_size(rect.right_bottom() - Vec2::splat(20.0), Vec2::splat(20.0));
        let r = ui.interact(h, id.with("size"), Sense::drag());
        if r.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
        }
        if r.drag_started() {
            let p = ui.ctx().input(|i| i.pointer.press_origin()).or(r.interact_pointer_pos()).unwrap_or(rect.right_bottom());
            acts.push(TileAct::StartResize { id: t.id, grab: p - rect.right_bottom() });
        }
        let col = if r.hovered() || active { accent } else { ui.visuals().weak_text_color() };
        let b = rect.right_bottom();
        for k in [5.0_f32, 10.0, 15.0] {
            painter.line_segment([Pos2::new(b.x - 3.0, b.y - k), Pos2::new(b.x - k, b.y - 3.0)], Stroke::new(1.3_f32, col));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::planning::projects::{create_project, get_project, NewProject};
    use crate::planning::views::dashboards::{create_dashboard, default_dashboard, list_dashboards, NewDashboard, TileKind};
    use eframe::egui::RawInput;

    /// Kleine Testumgebung: Datenbank im Speicher mit einem Projekt (Standardinhalte) und ein paar Tickets.
    struct Env {
        db: Db,
        project: i64,
    }

    fn env() -> Env {
        let conn = crate::db::open_memory();
        let mut np = NewProject::new("PAN", "Paneldeck", "#112233");
        np.with_defaults = true;
        let project = create_project(&conn, &np).unwrap();
        Env { db: Db { conn, path: std::path::PathBuf::new() }, project }
    }

    /// Ein Bild zeichnen: der Reiter in einem zentralen Bereich (wie in der Anwendung).
    fn frame(ctx: &egui::Context, plan: &mut PlanUi, e: &Env, lang: Lang, f: impl FnOnce(&mut PlanUi, &Cx)) {
        let proj = get_project(&e.db.conn, e.project).unwrap();
        let cx = Cx { conn: &e.db.conn, db: &e.db, lang };
        let mut f = Some(f);
        let _ = ctx.run(RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 800.0))), ..Default::default() }, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                plan.dash_tab(ui, &cx, &proj);
            });
            plan.dash_dialogs(ctx, &cx);
            if let Some(f) = f.take() {
                f(plan, &cx);
            }
        });
    }

    #[test]
    fn first_open_creates_the_default_dashboard_with_one_search_tile_and_loads_it() {
        let e = env();
        let ctx = egui::Context::default();
        let mut plan = PlanUi::default();
        frame(&ctx, &mut plan, &e, Lang::En, |_, _| {});
        let list = list_dashboards(&e.db.conn, Some(e.project)).unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].is_default);
        assert_eq!(list[0].layout.tiles.len(), 1);
        assert!(matches!(list[0].layout.tiles[0].kind, TileKind::Search { .. }));
        assert_eq!(plan.dash.cur, list[0].id);
        // das erste Bild lädt die Kachel in den Zwischenspeicher
        assert_eq!(plan.dash.cache.len(), 1);
        assert!(matches!(plan.dash.cache.values().next().map(|c| &c.data), Some(TileData::Rows { .. })));
        // nur einmal: nach dem Löschen aller Dashboards legt der Reiter kein neues an
        crate::planning::views::dashboards::delete_dashboard(&e.db.conn, list[0].id).unwrap();
        plan.dash_invalidate();
        frame(&ctx, &mut plan, &e, Lang::En, |_, _| {});
        assert!(list_dashboards(&e.db.conn, Some(e.project)).unwrap().is_empty());
        assert_eq!(plan.dash.cur, 0);
    }

    #[test]
    fn startup_prefers_the_default_dashboard_and_remembers_the_choice_in_the_session() {
        let e = env();
        let a = create_dashboard(&e.db.conn, &NewDashboard { name: "A".into(), project_id: Some(e.project), layout: None }).unwrap();
        let b = create_dashboard(&e.db.conn, &NewDashboard { name: "B".into(), project_id: Some(e.project), layout: None }).unwrap();
        set_default(&e.db.conn, b).unwrap();
        let ctx = egui::Context::default();
        let mut plan = PlanUi::default();
        frame(&ctx, &mut plan, &e, Lang::En, |_, _| {});
        assert_eq!(plan.dash.cur, b, "beim Start gilt das Standard-Dashboard");
        plan.dash_select(a);
        frame(&ctx, &mut plan, &e, Lang::En, |_, _| {});
        assert_eq!(plan.dash.cur, a);
        assert_eq!(default_dashboard(&e.db.conn, Some(e.project)).unwrap().unwrap().id, b);
        // Wechsel des Projekts und zurück behält die Wahl
        plan.dash.chosen.insert(999, 1);
        assert_eq!(plan.dash.chosen.get(&e.project), Some(&a));
    }

    #[test]
    fn tiles_with_broken_settings_show_an_error_but_do_not_crash() {
        let e = env();
        let mut layout = crate::planning::views::dashboards::default_dashboard_layout();
        if let TileKind::Search { jql, .. } = &mut layout.tiles[0].kind {
            *jql = "type = ".into();
        }
        let id = create_dashboard(&e.db.conn, &NewDashboard { name: "Broken".into(), project_id: Some(e.project), layout: Some(layout) }).unwrap();
        let ctx = egui::Context::default();
        let mut plan = PlanUi::default();
        plan.dash.chosen.insert(e.project, id);
        for lang in [Lang::De, Lang::En] {
            frame(&ctx, &mut plan, &e, lang, |_, _| {});
            frame(&ctx, &mut plan, &e, lang, |_, _| {});
            assert!(matches!(plan.dash.cache.values().next().map(|c| &c.data), Some(TileData::Error(m)) if !m.is_empty()));
        }
    }

    #[test]
    fn cache_is_not_reloaded_every_frame_but_after_changes_and_invalidation() {
        let e = env();
        let ctx = egui::Context::default();
        let mut plan = PlanUi::default();
        frame(&ctx, &mut plan, &e, Lang::En, |_, _| {});
        let loaded = |p: &PlanUi| p.dash.cache.values().next().map(|c| c.loaded).unwrap();
        let first = loaded(&plan);
        for _ in 0..3 {
            frame(&ctx, &mut plan, &e, Lang::En, |_, _| {});
        }
        assert_eq!(loaded(&plan), first, "unveränderte Kachel wird nicht jedes Bild neu gelesen");
        // Änderung von außen (z. B. Agent) → neu lesen
        plan.dash_invalidate();
        frame(&ctx, &mut plan, &e, Lang::En, |_, _| {});
        assert!(loaded(&plan) > first);
        // geänderte Sprache → andere Fehlertexte/Namen → neu lesen
        let second = loaded(&plan);
        frame(&ctx, &mut plan, &e, Lang::De, |_, _| {});
        assert!(loaded(&plan) > second);
    }

    #[test]
    fn many_tiles_load_a_few_per_frame() {
        let e = env();
        let mut layout = logic::overview_layout(Lang::En, false, false);
        layout.tiles.truncate(6);
        let id = create_dashboard(&e.db.conn, &NewDashboard { name: "Many".into(), project_id: Some(e.project), layout: Some(layout) }).unwrap();
        let ctx = egui::Context::default();
        let mut plan = PlanUi::default();
        plan.dash.chosen.insert(e.project, id);
        frame(&ctx, &mut plan, &e, Lang::En, |_, _| {});
        assert!(plan.dash.cache.len() <= MAX_LOADS, "höchstens {MAX_LOADS} Kacheln im ersten Bild: {}", plan.dash.cache.len());
        for _ in 0..5 {
            frame(&ctx, &mut plan, &e, Lang::En, |_, _| {});
        }
        assert_eq!(plan.dash.cache.len(), 6, "nach ein paar Bildern sind alle da");
    }

    #[test]
    fn edit_mode_renders_and_dialogs_open_without_panics() {
        let e = env();
        let ctx = egui::Context::default();
        let mut plan = PlanUi::default();
        frame(&ctx, &mut plan, &e, Lang::En, |p, _| p.dash.edit = true);
        frame(&ctx, &mut plan, &e, Lang::De, |_, _| {});
        let cur = plan.dash.cur;
        // jeder Dialog lässt sich öffnen und zeichnen
        for lang in [Lang::En, Lang::De] {
            frame(&ctx, &mut plan, &e, lang, |p, cx| p.open_tile_dialog(cx, None));
            frame(&ctx, &mut plan, &e, lang, |_, _| {});
            frame(&ctx, &mut plan, &e, lang, |p, cx| {
                p.dash.dlg = None;
                p.open_tile_dialog(cx, Some(1));
            });
            frame(&ctx, &mut plan, &e, lang, |_, _| {});
            frame(&ctx, &mut plan, &e, lang, |p, _| {
                p.dash.dlg = None;
                p.open_new_dashboard(&get_project(&e.db.conn, e.project).unwrap(), lang);
            });
            frame(&ctx, &mut plan, &e, lang, |_, _| {});
            frame(&ctx, &mut plan, &e, lang, |p, _| {
                p.dash.dlg = None;
                p.open_rename_dashboard(cur, "X".into());
            });
            frame(&ctx, &mut plan, &e, lang, |_, _| {});
            frame(&ctx, &mut plan, &e, lang, |p, _| {
                p.dash.dlg = None;
                p.open_delete_dashboard(cur, "X".into());
            });
            frame(&ctx, &mut plan, &e, lang, |_, _| {});
        }
        assert!(plan.dash.dlg.is_some());
    }

    #[test]
    fn tile_actions_change_and_persist_the_layout() {
        let e = env();
        let ctx = egui::Context::default();
        let mut plan = PlanUi::default();
        frame(&ctx, &mut plan, &e, Lang::En, |_, _| {});
        let id = plan.dash.cur;
        let stored = |e: &Env| crate::planning::views::dashboards::get_dashboard(&e.db.conn, id).unwrap().layout;
        // Kachel hinzufügen über den Entwurf
        frame(&ctx, &mut plan, &e, Lang::En, |p, cx| {
            let mut l = p.dash.current().unwrap().layout.clone();
            logic::apply_draft(&mut l, None, &TileDraft::new(KindSel::Flagged)).unwrap();
            assert!(p.dash_try_save(cx, id, l).is_ok());
        });
        assert_eq!(stored(&e).tiles.len(), 2);
        // Kachel verschieben (Pfeil) und entfernen
        frame(&ctx, &mut plan, &e, Lang::En, |p, cx| p.dash_tile_acts(cx, id, vec![TileAct::Nudge { id: 2, dx: 1, dy: 0 }]));
        assert_eq!(stored(&e).tile(2).unwrap().x, 1);
        frame(&ctx, &mut plan, &e, Lang::En, |p, cx| p.dash_tile_acts(cx, id, vec![TileAct::Remove(2)]));
        assert_eq!(stored(&e).tiles.len(), 1);
        // ein ungültiges Layout wird abgelehnt (Meldung statt Absturz) und der Speicher bleibt unverändert
        frame(&ctx, &mut plan, &e, Lang::En, |p, cx| {
            let mut l = p.dash.current().unwrap().layout.clone();
            l.tiles[0].kind = TileKind::Search { jql: "x".repeat(10_000), columns: vec![], title: String::new() };
            assert!(p.dash_try_save(cx, id, l).is_err());
        });
        assert_eq!(stored(&e).tiles.len(), 1);
        assert!(plan.dash.stale, "nach einem Fehler wird die Liste neu gelesen");
    }

    #[test]
    fn global_view_in_the_system_project_lists_only_global_dashboards() {
        let e = env();
        let global = create_dashboard(&e.db.conn, &NewDashboard { name: "All".into(), project_id: None, layout: Some(logic::overview_layout(Lang::En, false, true)) }).unwrap();
        let system = crate::planning::projects::list_projects(&e.db.conn).unwrap().into_iter().find(|p| p.system).unwrap();
        let ctx = egui::Context::default();
        let mut plan = PlanUi::default();
        let cx = Cx { conn: &e.db.conn, db: &e.db, lang: Lang::En };
        for _ in 0..4 {
            let _ = ctx.run(RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 800.0))), ..Default::default() }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| plan.dash_tab(ui, &cx, &system));
            });
        }
        assert_eq!(plan.dash.cur, global);
        assert!(plan.dash.list.iter().all(|d| d.project_id.is_none()));
        assert!(!plan.dash.cache.is_empty());
        // in einem Projekt sind die globalen ebenfalls wählbar
        let mut plan2 = PlanUi::default();
        let proj = get_project(&e.db.conn, e.project).unwrap();
        let _ = ctx.run(RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| plan2.dash_tab(ui, &cx, &proj));
        });
        assert!(plan2.dash.list.iter().any(|d| d.id == global));
    }

    #[test]
    fn extreme_layouts_do_not_break_drawing() {
        use crate::planning::views::dashboards::{ChartKind, CountGroup, MAX_TILES};
        let e = env();
        // sehr viele winzige Kacheln
        let mut layout = Layout::default();
        for i in 0..MAX_TILES as u32 {
            let kind = match i % 4 {
                0 => TileKind::Favorites,
                1 => TileKind::Projects,
                2 => TileKind::Counts { jql: String::new(), group_by: CountGroup::Status, chart: ChartKind::Pie },
                _ => TileKind::Flagged,
            };
            layout.tiles.push(Tile { id: i + 1, x: i % 12, y: (i / 12) * 2, w: 1, h: 1, kind });
        }
        layout.normalize();
        let id = create_dashboard(&e.db.conn, &NewDashboard { name: "Tiny".into(), project_id: Some(e.project), layout: Some(layout) }).unwrap();
        let ctx = egui::Context::default();
        let mut plan = PlanUi::default();
        plan.dash.chosen.insert(e.project, id);
        for edit in [false, true] {
            plan.dash.edit = edit;
            for _ in 0..25 {
                frame(&ctx, &mut plan, &e, Lang::De, |_, _| {});
            }
        }
        assert_eq!(plan.dash.cache.len(), MAX_TILES);
    }

    #[test]
    fn a_database_file_is_read_by_the_background_worker() {
        let dir = std::env::temp_dir().join(format!("paneldeck-dash-test-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)));
        std::fs::create_dir_all(&dir).unwrap();
        let db = crate::db::open(&dir, &dir.join("backup")).unwrap();
        let mut np = NewProject::new("PAN", "Paneldeck", "#112233");
        np.with_defaults = true;
        let project = create_project(&db.conn, &np).unwrap();
        let types = crate::planning::types::list_types(&db.conn, project).unwrap();
        for i in 0..3 {
            crate::planning::tickets::create_ticket(&db.conn, &crate::planning::tickets::NewTicket::new(project, types[0].id, &format!("T{i}")), &crate::planning::history::Source::ui()).unwrap();
        }
        let layout = logic::overview_layout(Lang::En, false, false);
        let n = layout.tiles.len();
        let id = create_dashboard(&db.conn, &NewDashboard { name: "Async".into(), project_id: Some(project), layout: Some(layout) }).unwrap();
        let e = Env { db, project };
        let ctx = egui::Context::default();
        let mut plan = PlanUi::default();
        plan.dash.chosen.insert(project, id);
        frame(&ctx, &mut plan, &e, Lang::En, |_, _| {});
        // das erste Bild lädt nichts im Vordergrund: die Aufträge sind beim Hintergrund-Leser
        assert!(plan.dash.worker.is_some());
        assert!(plan.dash.cache.len() < n, "im ersten Bild ist noch nicht alles da");
        assert!(!plan.dash.inflight.is_empty());
        // nach ein paar Bildern sind alle Ergebnisse da (der Faden weckt die Oberfläche; im Test fragen wir selbst nach)
        let started = Instant::now();
        while plan.dash.cache.len() < n && started.elapsed() < Duration::from_secs(20) {
            std::thread::sleep(Duration::from_millis(20));
            frame(&ctx, &mut plan, &e, Lang::En, |_, _| {});
        }
        assert_eq!(plan.dash.cache.len(), n, "alle Kacheln über den Hintergrund-Leser geladen");
        assert!(plan.dash.inflight.is_empty());
        let counts = plan.dash.cache.values().find_map(|c| match &c.data {
            TileData::Counts { total, .. } => Some(*total),
            _ => None,
        });
        assert_eq!(counts, Some(3), "der Leser sieht die Daten der Datei");
        assert!(!plan.dash.cache.values().any(|c| matches!(c.data, TileData::Error(_))), "keine Fehler beim Lesen");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pending_tiles_follow_the_cache_rules() {
        let e = env();
        let layout = logic::overview_layout(Lang::En, true, false);
        let mut st = State::default();
        let now = Instant::now();
        assert_eq!(st.pending(&layout, Some(e.project), Lang::En, now).len(), layout.tiles.len(), "ohne Daten: alle");
        let t = &layout.tiles[0];
        st.cache.insert(t.id, Cached { meta: CacheMeta { key: logic::tile_key(&t.kind, Some(e.project), Lang::En), generation: st.generation }, loaded: now, data: TileData::Info(String::new()) });
        let p = st.pending(&layout, Some(e.project), Lang::En, now);
        assert!(!p.iter().any(|(id, _)| *id == t.id), "frisch: nicht neu laden");
        assert!(st.pending(&layout, Some(e.project), Lang::De, now).iter().any(|(id, has)| *id == t.id && *has), "Sprache geändert: neu laden, Daten bleiben bis dahin sichtbar");
        assert!(st.pending(&layout, Some(e.project), Lang::En, now + TTL + Duration::from_secs(1)).iter().any(|(id, _)| *id == t.id), "abgelaufen");
        st.invalidate();
        assert!(st.pending(&layout, Some(e.project), Lang::En, now).iter().any(|(id, _)| *id == t.id), "Stand geändert");
    }
}
