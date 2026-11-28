//! Einstellungen (#395: eigenes Fenster mit Kategorienbaum, Suche, Marken und Zurücksetzen), Datei-Editor, Schrift.
//!
//! Aufbau: Baum und Suche in `settings_nav`, die Beschreibung jeder Einstellung (Beschriftung, Stichwörter, Standard) in
//! `settings_items`; hier das Fenster, Kopf und Fuß der Seite sowie der Inhalt der Seiten.

use super::settings_items::{self as items, block, link, note, srow, srow_hint, RowCx};
use super::settings_nav::{self as nav, NavKey, Page, Query, TreeView};
use super::*;
use std::collections::BTreeSet;

/// Größe des Fensters beim ersten Öffnen (danach die zuletzt gesehene, nur zur Laufzeit).
const DEFAULT_SIZE: Vec2 = Vec2::new(900.0, 620.0);

impl App {
    /// Einstellungen bei Seite `p` öffnen (Sprungziel); sind sie schon offen, wechselt die Seite und das Fenster kommt nach vorn.
    pub(super) fn open_settings(&mut self, p: Page) {
        self.settings_open = true;
        self.settings_nav.open_at(p);
        self.settings_nav.was_open = true;
    }

    /// Einstellungen auf- bzw. zuklappen (Fensterleiste, Palette): öffnet immer bei der Startseite.
    pub(super) fn toggle_settings(&mut self) {
        if self.settings_open {
            self.settings_open = false;
        } else {
            self.open_settings(Page::START);
        }
    }

    /// Zeichnen die Einstellungen in den Hub statt in ein eigenes Fenster? Während einer Tour (das Tutorial zeigt nur
    /// im Hauptfenster auf Ziele) und für die Doku-Bilder (die Aufnahme zeigt nur das Hauptfenster).
    pub(super) fn settings_embedded(&self) -> bool {
        cfg!(feature = "readme-shots") || (self.tut_running() && self.wins.cur == 0)
    }

    /// Die Einstellungen sind als eigenes Fenster offen: ihre Rückfragen (Wiederherstellen, Zurücksetzen) zeichnen dort.
    pub(super) fn settings_owns_dialogs(&self) -> bool {
        self.settings_open && !self.settings_embedded()
    }

    pub(super) fn settings_ui(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            // Eine nicht abgeschlossene Kürzel-Aufnahme würde sonst alle Tastenkürzel dauerhaft sperren.
            self.capture_key = None;
            if self.remember_off_ask == Some(self.wins.cur) {
                self.remember_off_ask = None; // Einstellungen geschlossen: Rückfrage gilt als abgebrochen
            }
            if self.settings_nav.was_open {
                // Beim nächsten Öffnen wieder auf der Startseite (#395), Lage wie zuletzt gesehen.
                let geo = self.settings_nav.geo;
                self.settings_nav = nav::Nav { geo, ..nav::Nav::default() };
            }
            return;
        }
        if !self.settings_nav.was_open {
            self.settings_nav.was_open = true;
            self.settings_nav.focus_search = true;
        }
        if self.settings_embedded() {
            self.settings_in_hub(ctx);
        } else {
            let lang = self.settings.language;
            let hub = ctx.input(|i| i.viewport().outer_rect);
            let geo = self.settings_nav.geo;
            // Lage beim Öffnen: zuletzt gesehene, sonst mittig über dem Hub; bleibt, solange das Fenster offen ist.
            let (pos, size) = *self.settings_nav.init.get_or_insert_with(|| match geo {
                Some((p, s)) => (Some(p), s),
                None => (hub.map(|r| r.center() - DEFAULT_SIZE / 2.0), DEFAULT_SIZE),
            });
            let mut b = egui::ViewportBuilder::default()
                .with_title(format!("Paneldeck – {}", lang.t("Einstellungen", "Settings")))
                .with_inner_size(size)
                .with_min_inner_size([420.0, 320.0])
                .with_icon(super::multiwin::icon());
            if let Some(p) = pos {
                b = b.with_position(p);
            }
            let vid = egui::ViewportId::from_hash_of(("settings_win", self.wins.cur));
            ctx.show_viewport_immediate(vid, b, |vctx, class| {
                if class == egui::ViewportClass::Embedded {
                    self.settings_in_hub(vctx);
                } else {
                    self.settings_os_frame(vctx);
                }
            });
        }
        if !self.settings_open || self.settings_nav.page != Page::Shortcuts {
            self.capture_key = None;
        }
    }

    /// Ein Bild des eigenen Fensters (immer deckend, ohne Hintergrund des Themes).
    fn settings_os_frame(&mut self, ctx: &egui::Context) {
        let backdrop = style::backdrop_on();
        style::set_backdrop(false);
        let (close, outer, inner) = ctx.input(|i| {
            let v = i.viewport();
            (v.close_requested(), v.outer_rect, v.inner_rect)
        });
        if let (Some(o), Some(i)) = (outer, inner) {
            self.settings_nav.geo = Some((o.min, i.size()));
        }
        if std::mem::take(&mut self.settings_nav.raise) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        let fill = style::ui().window_bg.to_opaque();
        egui::CentralPanel::default().frame(egui::Frame::new().fill(fill)).show(ctx, |ui| self.settings_body(ui, true));
        self.settings_dialogs(ctx);
        if close {
            self.settings_open = false;
        }
        style::set_backdrop(backdrop);
    }

    /// Im Hub statt eines eigenen Fensters (Tour, Doku-Bilder, Oberflächen ohne weitere Fenster).
    fn settings_in_hub(&mut self, ctx: &egui::Context) {
        let lang = self.settings.language;
        let scr = ctx.content_rect();
        let size = Vec2::new(DEFAULT_SIZE.x.min(scr.width() - 40.0), DEFAULT_SIZE.y.min(scr.height() - 80.0)).max(Vec2::new(320.0, 240.0));
        let mut open = true;
        style::movable(ctx, Id::new("settings_win"), egui::Window::new(lang.t("Einstellungen", "Settings")))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .fixed_size(size)
            .show(ctx, |ui| {
                ui.set_min_size(size);
                self.settings_body(ui, false);
            });
        self.settings_dialogs(ctx);
        if !open {
            self.settings_open = false;
        }
    }

    /// Rückfragen, die zu den Einstellungen gehören (im selben Fenster wie sie).
    fn settings_dialogs(&mut self, ctx: &egui::Context) {
        self.remember_off_ui(ctx);
        if self.settings_owns_dialogs() {
            self.restore_dialog(ctx);
        }
        let Some(keys) = self.settings_nav.ask.clone() else { return };
        let lang = self.settings.language;
        let (mut go, mut cancel) = (false, ctx.input(|i| i.key_pressed(Key::Escape)));
        style::movable(ctx, Id::new("settings_reset_ask"), egui::Window::new(lang.t("Zurücksetzen?", "Reset?")))
            .collapsible(false)
            .resizable(false)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.label(lang.t("Diese Einstellungen gehen auf ihren Standard zurück:", "These settings go back to their defaults:"));
                for k in &keys {
                    if let Some(i) = items::item(k) {
                        let mark = if i.risky { "  ⚠" } else { "" };
                        ui.label(format!("• {}{mark}", i.label(lang)));
                    }
                }
                ui.add_space(S1);
                ui.label(RichText::new(lang.t(
                    "⚠ = betrifft Sicherheit, Zugriffe, Windows-Integration oder gespeicherte Sitzungen.",
                    "⚠ = affects security, access, Windows integration or saved sessions.",
                )).small().color(style::dim(ui)));
                ui.add_space(S2);
                ui.horizontal(|ui| {
                    go = ui.button(lang.t("Zurücksetzen", "Reset")).clicked();
                    cancel |= ui.button(lang.t("Abbrechen", "Cancel")).clicked();
                });
            });
        if go {
            self.settings_reset_apply(&keys);
        }
        if go || cancel {
            self.settings_nav.ask = None;
        }
    }

    /// Zurücksetzen anfordern: riskante Einträge fragen erst nach.
    fn settings_reset_request(&mut self, keys: Vec<&'static str>) {
        if keys.is_empty() {
            return;
        }
        if keys.iter().any(|k| items::item(k).is_some_and(|i| i.risky)) {
            self.settings_nav.ask = Some(keys);
        } else {
            self.settings_reset_apply(&keys);
        }
    }

    /// Einträge auf ihren Standard setzen und sofort sichern (Sicherheitseinstellungen dürfen nicht warten).
    fn settings_reset_apply(&mut self, keys: &[&'static str]) {
        for k in items::reset_settings(&mut self.settings, keys) {
            if let Some(items::Def::A { reset: Some(r), .. }) = items::item(k).map(|i| &i.def) {
                r(self);
            }
        }
        self.scale_pending = None;
        self.capture_key = None;
        self.editor_svc.excludes_buf = None;
        self.mcp_reach_dirty();
        state::save_json(&self.dir.join("settings.json"), &self.settings);
    }

    /// Autostart ausschalten (Zurücksetzen).
    pub(super) fn reset_autostart(&mut self) {
        let exe = self.exe.clone();
        match crate::integration::set_autostart(false, &exe) {
            Ok(()) => self.win_autostart = crate::integration::Status::Off,
            Err(e) => self.hooks_msg = e,
        }
    }

    /// Ordner-Kontextmenü entfernen (Zurücksetzen).
    pub(super) fn reset_context_menu(&mut self) {
        let exe = self.exe.clone();
        let shown = state::lang();
        state::set_lang(state::persisted_lang(self.settings.language));
        let r = crate::integration::set_context(false, &exe);
        state::set_lang(shown);
        match r {
            Ok(()) => self.win_context = crate::integration::Status::Off,
            Err(e) => self.hooks_msg = e,
        }
    }

    /// Eintrag „In Paneldeck-Editor öffnen“ entfernen (Zurücksetzen).
    pub(super) fn reset_edit_context(&mut self) {
        let exe = self.exe.clone();
        match crate::integration::set_edit_context(false, &exe) {
            Ok(()) => self.editor_svc.edit_context = crate::integration::Status::Off,
            Err(e) => self.hooks_msg = e,
        }
    }

    /// Globale Hooks entfernen (Zurücksetzen).
    pub(super) fn reset_hooks_global(&mut self) {
        self.hooks_msg = match hooks::global_settings_path().map(|p| hooks::uninstall_global(&p)) {
            Some(Ok(())) => String::new(),
            Some(Err(e)) => e,
            None => state::t("~/.claude nicht gefunden", "~/.claude not found").into(),
        };
        self.refresh_hooks_global();
    }

    /// Offene Fenster des Hubs, die eine Seite öffnen kann (Aufräumen, Theme-Editor, Regeln beim Speichern): geht eines
    /// auf, kommt der Hub nach vorn – sonst läge es hinter dem Einstellungsfenster.
    fn hub_dialog_sig(&self) -> [bool; 4] {
        let sa = &self.save_actions.ui;
        [self.cleanup.is_some(), self.theme_ui.any_open(), sa.editor.is_some() || sa.test.is_some() || sa.import.is_some(), sa.output_open]
    }

    /// Inhalt des Fensters: Baum links (schmal: Auswahl oben), Seite rechts.
    fn settings_body(&mut self, ui: &mut Ui, os_window: bool) {
        let lang = self.settings.language;
        let q = Query::new(&self.settings_nav.query);
        let found = (!q.is_empty()).then(|| items::search(&q, lang));
        let modified = items::modified_keys(&self.settings, Some(self));
        let mod_pages = items::per_page(&modified);
        let filter: Option<BTreeSet<Page>> = match &found {
            Some(f) => Some(f.pages()),
            None if self.settings_nav.only_modified => Some(mod_pages.keys().copied().collect()),
            None => None,
        };
        // Die gezeigte Seite liegt immer im Filter (Suche springt zur ersten Seite mit Treffern).
        if let Some(f) = &filter
            && !f.contains(&self.settings_nav.page)
            && let Some(first) = Page::ALL.iter().find(|p| f.contains(p))
        {
            self.settings_nav.select(*first);
        }
        self.settings_keys(ui.ctx(), filter.as_ref(), os_window);
        let badges: BTreeMap<Page, usize> = match &found {
            Some(f) => f.per_page.clone(),
            None => mod_pages.clone(),
        };
        let view = TreeView { lang, filter: filter.as_ref(), badges: &badges, accent_badges: found.is_none() };
        let full = ui.available_rect_before_wrap();
        let narrow = full.width() < nav::NARROW_W;
        let right = if narrow {
            let top = Rect::from_min_size(full.min, Vec2::new(full.width(), if found.is_some() { 92.0 } else { 74.0 }));
            let mut t = ui.new_child(UiBuilder::new().id_salt("settings_top").max_rect(top.shrink2(Vec2::new(12.0, 8.0))).layout(egui::Layout::top_down(egui::Align::LEFT)));
            nav::search_box(&mut t, &mut self.settings_nav, lang);
            if let Some(f) = &found {
                t.label(RichText::new(hits_text(lang, f.total(), f.per_page.len())).small().color(style::dim(&t)));
            }
            nav::picker(&mut t, &mut self.settings_nav, &view);
            Rect::from_min_max(Pos2::new(full.left(), top.bottom()), full.max)
        } else {
            let left = Rect::from_min_size(full.min, Vec2::new(nav::TREE_W, full.height()));
            ui.painter().rect_filled(left, 0.0, style::ui().sidebar_bg.to_opaque());
            ui.painter().vline(left.right(), left.y_range(), Stroke::new(1.0_f32, style::ui().border));
            self.settings_left(ui, left, &view, found.as_ref(), modified.len());
            Rect::from_min_max(Pos2::new(left.right() + 1.0, full.top()), full.max)
        };
        let page = self.settings_nav.page;
        let show_all = self.settings_nav.show_all == Some(page);
        let cx = RowCx::new(lang, page, q, found.as_ref(), modified, self.settings_nav.only_modified && found.is_none(), show_all);
        let mut r = ui.new_child(UiBuilder::new().id_salt("settings_right").max_rect(right.shrink2(Vec2::new(18.0, 10.0))).layout(egui::Layout::top_down(egui::Align::LEFT)));
        let tut_cmd = self.settings_right(&mut r, &cx, found.is_some(), filter.as_ref().is_some_and(|f| f.is_empty()));
        ui.advance_cursor_after_rect(full);
        if let Some(k) = cx.reset.get() {
            self.settings_reset_request(vec![k]);
        }
        if cx.reset_page.get() {
            self.settings_reset_request(cx.page_resettable());
        }
        if let Some(p) = cx.go.get() {
            self.settings_nav.select(p);
        }
        self.settings_nav.last_focus = ui.ctx().memory(|m| m.focused());
        if let Some(c) = tut_cmd {
            let ctx = ui.ctx().clone();
            self.tut_cmd(&ctx, c);
        }
    }

    /// Linke Spalte: Suchfeld, Trefferzahl, Baum, unten „Nur geänderte“ bzw. „Esc leert die Suche“.
    fn settings_left(&mut self, ui: &mut Ui, left: Rect, view: &TreeView, found: Option<&items::Found>, modified: usize) {
        let lang = view.lang;
        let inner = left.shrink2(Vec2::new(10.0, 12.0));
        let bottom_h = 30.0;
        let mut top = ui.new_child(UiBuilder::new().id_salt("settings_tree").max_rect(Rect::from_min_max(inner.min, Pos2::new(inner.right(), inner.bottom() - bottom_h))).layout(egui::Layout::top_down(egui::Align::LEFT)));
        nav::search_box(&mut top, &mut self.settings_nav, lang);
        top.add_space(S1);
        if let Some(f) = found {
            top.label(RichText::new(hits_text(lang, f.total(), f.per_page.len())).small().color(style::dim(&top)));
        }
        egui::ScrollArea::vertical().id_salt("settings_tree_scroll").auto_shrink([false, false]).show(&mut top, |ui| {
            nav::tree(ui, &mut self.settings_nav, view);
        });
        let mut bottom = ui.new_child(UiBuilder::new().id_salt("settings_tree_bottom").max_rect(Rect::from_min_max(Pos2::new(inner.left(), inner.bottom() - bottom_h + 6.0), inner.max)).layout(egui::Layout::left_to_right(egui::Align::Center)));
        if found.is_some() {
            bottom.label(RichText::new(lang.t("Esc leert die Suche", "Esc clears the search")).small().color(style::dim(&bottom)));
        } else {
            let text = format!("{} ({modified})", lang.t("Nur geänderte", "Only modified"));
            bottom.checkbox(&mut self.settings_nav.only_modified, text).on_hover_text(lang.t("Nur Einstellungen zeigen, die vom Standard abweichen", "Show only settings that differ from their defaults"));
        }
    }

    /// Rechte Spalte: Kopf (Pfad, Zurücksetzen bzw. Trefferzahl), Seite, Fuß mit Tastenhinweisen.
    fn settings_right(&mut self, ui: &mut Ui, cx: &RowCx, searching: bool, nothing: bool) -> Option<tutorial::Cmd> {
        let lang = cx.lang;
        let page = cx.page;
        let area = ui.max_rect();
        let head = Rect::from_min_size(area.min, Vec2::new(area.width(), 40.0));
        let foot_h = 22.0;
        let mut h = ui.new_child(UiBuilder::new().id_salt("settings_head").max_rect(head).layout(egui::Layout::left_to_right(egui::Align::Center)));
        if nothing {
            h.label(RichText::new(lang.t("Keine Treffer", "No results")).size(17.0));
        } else {
            // Pfad: der Bereich ist anklickbar (zeigt seine erste Seite).
            if !page.group().single() {
                let g = page.group();
                let r = h.add(egui::Label::new(RichText::new(g.title(lang)).size(17.0).color(style::dim(&h))).sense(Sense::click()));
                if r.hovered() {
                    h.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if r.clicked()
                    && let Some(first) = g.pages().next()
                {
                    self.settings_nav.select(first);
                }
                h.label(RichText::new("›").size(17.0).color(style::dim(&h)));
            }
            h.label(RichText::new(page.title(lang)).size(17.0).strong());
            h.with_layout(egui::Layout::right_to_left(egui::Align::Center), |h| {
                if searching {
                    let (n, all) = cx.counts();
                    let t = match lang {
                        Lang::De => format!("{n} von {all} Einstellungen"),
                        Lang::En => format!("{n} of {all} settings"),
                    };
                    h.label(RichText::new(t).small().color(style::dim(h)));
                } else {
                    let keys = cx.page_resettable();
                    let b = egui::Button::new(format!("↺ {}", lang.t("Seite zurücksetzen", "Reset page")));
                    let tip = lang.t("Alle geänderten Einstellungen dieser Seite auf den Standard setzen (riskante fragen erst nach)", "Set all changed settings on this page back to their defaults (risky ones ask first)");
                    if h.add_enabled(!keys.is_empty(), b).on_hover_text(tip).clicked() {
                        cx.reset_page.set(true);
                    }
                }
            });
        }
        ui.painter().hline(area.x_range(), head.bottom() + 2.0, Stroke::new(1.0_f32, style::ui().border));
        let body = Rect::from_min_max(Pos2::new(area.left(), head.bottom() + 8.0), Pos2::new(area.right(), area.bottom() - foot_h - 4.0));
        let mut b = ui.new_child(UiBuilder::new().id_salt("settings_page_area").max_rect(body).layout(egui::Layout::top_down(egui::Align::LEFT)));
        let mut tut_cmd = None;
        if nothing {
            b.label(RichText::new(lang.t("Nichts gefunden. Deutsch und Englisch werden beide durchsucht, dazu Erklärungen und Stichwörter.", "Nothing found. German and English are both searched, as well as hints and keywords.")).color(style::dim(&b)));
        } else {
            egui::ScrollArea::vertical().id_salt(("settings_page", page.id())).auto_shrink([false, false]).show(&mut b, |ui| {
                let before = self.hub_dialog_sig();
                tut_cmd = self.settings_page(ui, cx);
                if self.hub_dialog_sig() != before && self.settings_owns_dialogs() {
                    // Fenster des Hubs (Aufräumen, Theme-Editor, Regeln) liegen sonst hinter den Einstellungen.
                    ui.ctx().send_viewport_cmd_to(super::multiwin::viewport_of(self.wins.cur), egui::ViewportCommand::Focus);
                }
                // Ausgeblendete Zeilen (Suche) mit „Alle zeigen“.
                let hidden = cx.hidden();
                if searching && !hidden.is_empty() {
                    ui.add_space(S2);
                    ui.horizontal_wrapped(|ui| {
                        ui.add_space(items::GUTTER);
                        let t = match lang {
                            Lang::De => format!("{}: ausgeblendet (kein Treffer)", hidden.join(", ")),
                            Lang::En => format!("{}: hidden (no match)", hidden.join(", ")),
                        };
                        ui.label(RichText::new(t).small().color(style::dim(ui)));
                        if ui.link(RichText::new(lang.t("Alle zeigen", "Show all")).small()).clicked() {
                            self.settings_nav.show_all = Some(page);
                        }
                    });
                }
                ui.add_space(S3);
            });
        }
        let foot = Rect::from_min_max(Pos2::new(area.left(), area.bottom() - foot_h), area.max);
        let hint = if searching {
            lang.t(
                "Deutsch und Englisch werden immer beide gefunden („opacity“ findet „Deckkraft“), dazu gepflegte Stichwörter",
                "German and English always both match (“Deckkraft” finds “Opacity”), plus curated keywords",
            )
        } else {
            lang.t(
                "Änderungen gelten sofort · ↑↓ Seite, ←→ auf/zu, Tab ins Formular, Strg+Tab nächste Seite · Esc schließt",
                "Changes apply instantly · ↑↓ page, ←→ expand/collapse, Tab into the form, Ctrl+Tab next page · Esc closes",
            )
        };
        let mut f = ui.new_child(UiBuilder::new().id_salt("settings_foot").max_rect(foot).layout(egui::Layout::left_to_right(egui::Align::Center)));
        f.add(egui::Label::new(RichText::new(hint).small().color(style::dim(&f))).truncate());
        tut_cmd
    }

    /// Tasten des Fensters: Strg+F Suche, Strg+Tab Seite, Esc erst Suche leeren, dann schließen; Pfeile im Baum.
    fn settings_keys(&mut self, ctx: &egui::Context, filter: Option<&BTreeSet<Page>>, os_window: bool) {
        // Kürzel-Aufnahme und Rückfragen haben die Tastatur.
        if self.capture_key.is_some() || self.settings_nav.ask.is_some() || self.remember_off_ask.is_some() || (self.settings_owns_dialogs() && self.confirm_restore.is_some()) {
            return;
        }
        let n = &mut self.settings_nav;
        let search = nav::search_id();
        let focused = ctx.memory(|m| m.focused());
        let (find, next, prev) = ctx.input_mut(|i| (i.consume_key(Modifiers::CTRL, Key::F), i.consume_key(Modifiers::CTRL, Key::Tab), i.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::Tab)));
        if find {
            n.focus_search = true;
        }
        if next {
            n.step(1, filter);
        }
        if prev {
            n.step(-1, filter);
        }
        // Esc: egui hat ein anderes Feld schon verlassen bzw. ein Menü geschlossen – dann nichts weiter.
        let other_field = n.last_focus.is_some_and(|f| f != search);
        if ctx.input(|i| i.key_pressed(Key::Escape)) && !egui::Popup::is_any_open(ctx) && !other_field {
            if n.query.is_empty() {
                self.settings_open = false;
            } else {
                n.query.clear();
                n.focus_search = true;
            }
            return;
        }
        if !os_window || !(focused.is_none() || focused == Some(search)) {
            return;
        }
        let in_search = focused == Some(search);
        let keys = ctx.input(|i| {
            [
                (i.key_pressed(Key::ArrowUp), NavKey::Up),
                (i.key_pressed(Key::ArrowDown), NavKey::Down),
                (!in_search && i.key_pressed(Key::ArrowLeft), NavKey::Left),
                (!in_search && i.key_pressed(Key::ArrowRight), NavKey::Right),
            ]
        });
        for (pressed, k) in keys {
            if pressed {
                n.key(k, filter);
            }
        }
    }

    /// Inhalt einer Seite. Liefert einen Befehl des Tutorials (Start aus *Info › Tutorial*).
    fn settings_page(&mut self, ui: &mut Ui, cx: &RowCx) -> Option<tutorial::Cmd> {
        match cx.page {
            Page::GeneralStart => self.page_general_start(ui, cx),
            Page::GeneralNotifications => self.page_notifications(ui, cx),
            Page::GeneralWindow => self.page_window(ui, cx),
            Page::GeneralIntegration => self.page_integration(ui, cx),
            Page::AppearanceTheme => self.page_theme(ui, cx),
            Page::AppearanceFonts => self.page_fonts(ui, cx),
            Page::AppearanceTransparency => self.page_transparency(ui, cx),
            Page::AppearancePalettes => {
                let lang = cx.lang;
                block(ui, cx, "palette", false, |ui| palette_ui::settings_view(ui, &mut self.settings.palette, lang));
            }
            Page::TerminalShell => self.page_shell(ui, cx),
            Page::TerminalBehavior => self.page_behavior(ui, cx),
            Page::TerminalClipboard => self.page_clipboard(ui, cx),
            Page::TerminalProfiles => block(ui, cx, "profiles", false, |ui| self.profiles_ui(ui)),
            Page::TerminalRemote => {
                let lang = cx.lang;
                block(ui, cx, "remote_snippets", false, |ui| terminal_tab(ui, lang));
            }
            Page::EditorGeneral => self.editor_general_page(ui, cx),
            Page::EditorSearchFormat => self.editor_search_format_page(ui, cx),
            Page::EditorSyntax => self.editor_languages_page(ui, cx),
            Page::EditorSmart => self.editor_smart_page(ui, cx),
            Page::EditorDiff => self.editor_compare_page(ui, cx),
            Page::EditorOnSave => {
                block(ui, cx, "save_actions", false, |ui| self.save_actions_settings_ui(ui));
                link(ui, cx, cx.lang.t("Formatieren ohne Regel:", "Formatting without a rule:"), Page::EditorSearchFormat);
            }
            Page::Shortcuts => self.page_shortcuts(ui, cx),
            Page::ClaudeGeneral => self.page_claude_general(ui, cx),
            Page::ClaudeSession => self.page_claude_defaults(ui, cx, opts_tabs::Tab::Claude),
            Page::ClaudeAccess => self.page_claude_defaults(ui, cx, opts_tabs::Tab::Access),
            Page::ClaudeAdvanced => self.page_claude_defaults(ui, cx, opts_tabs::Tab::Advanced),
            Page::ClaudeCosts => self.page_claude_costs(ui, cx),
            Page::ClaudeHooks => self.page_claude_hooks(ui, cx),
            Page::ModelsServers => self.models_servers_page(ui, cx),
            Page::ModelsTools => self.models_tools_page(ui, cx),
            Page::ModelsCompact => self.models_compact_page(ui, cx),
            Page::AgentsAccess => self.agents_access_page(ui, cx),
            Page::AgentsEditor => self.editor_agents_page(ui, cx),
            Page::AgentsSetup => self.agents_setup_page(ui, cx),
            Page::AgentsLog => self.agents_log_page(ui, cx),
            Page::DataBackup => self.data_backup_page(ui, cx),
            Page::DataRestore => self.data_restore_page(ui, cx),
            Page::DataMaintenance => self.data_maintenance_page(ui, cx),
            Page::DataDiagnostics => self.page_diagnostics(ui, cx),
            Page::AboutVersion => self.page_about(ui, cx),
            Page::AboutUpdates => self.update_about_ui(ui, cx),
            Page::AboutTutorial => return self.tut_about_ui(ui, cx),
            Page::AboutLicenses => {
                block(ui, cx, "licenses", false, |ui| {
                    egui::ScrollArea::vertical().max_height((ui.ctx().content_rect().height() - 200.0).max(200.0)).id_salt("lic").show(ui, |ui| {
                        ui.label(RichText::new(include_str!("../../THIRD_PARTY.md")).small().monospace());
                    });
                });
            }
        }
        None
    }

    // ---------------------------------------------------------------------------------------------- Allgemein

    fn page_general_start(&mut self, ui: &mut Ui, cx: &RowCx) {
        let lang = cx.lang;
        srow(ui, cx, "language", |ui| {
            let a = afford::choice_value(ui, &mut self.settings.language, Lang::De, "Deutsch", 0.0);
            let b = afford::choice_value(ui, &mut self.settings.language, Lang::En, "English", 0.0);
            // Eine eigene Wahl beendet die Überschreibung durch `--lang` (sie wird dann gespeichert).
            if a.clicked() || b.clicked() {
                state::clear_lang_override();
            }
        });
        srow(ui, cx, "remember_sessions", |ui| {
            // Ausschalten erst nach Rückfrage (verwirft beim nächsten Beenden Projekte und Ordner).
            let mut on = self.settings.remember_sessions;
            style::toggle(ui, &mut on);
            if on && !self.settings.remember_sessions {
                self.settings.remember_sessions = true;
            } else if !on && self.settings.remember_sessions {
                self.remember_off_ask = Some(self.wins.cur);
            }
        });
        ui.add_space(S2);
        note(ui, cx, lang.t("Strg+Umschalt+P öffnet die Befehls-Palette – dort steht auch jede Seite dieser Einstellungen („Einstellungen: …“).", "Ctrl+Shift+P opens the command palette – it also lists every page of these settings (“Settings: …”)."));
        link(ui, cx, lang.t("Tastenkürzel ändern:", "Change keyboard shortcuts:"), Page::Shortcuts);
        link(ui, cx, lang.t("Standard-Shell und Umgebung:", "Default shell and environment:"), Page::TerminalShell);
    }

    fn page_notifications(&mut self, ui: &mut Ui, cx: &RowCx) {
        let s = &mut self.settings;
        srow(ui, cx, "notify_toast", |ui| {
            style::toggle(ui, &mut s.notify_toast);
        });
        srow(ui, cx, "notify_sound", |ui| {
            style::toggle(ui, &mut s.notify_sound);
        });
        srow(ui, cx, "notify_flash", |ui| {
            style::toggle(ui, &mut s.notify_flash);
        });
        srow(ui, cx, "cmd_notify_secs", |ui| {
            ui.add(egui::DragValue::new(&mut s.cmd_notify_secs).range(0..=3600).speed(1.0).suffix(" s"));
        });
        srow(ui, cx, "program_notify", |ui| {
            style::toggle(ui, &mut s.program_notify);
        });
        srow(ui, cx, "bell_toast", |ui| {
            style::toggle(ui, &mut s.bell_toast);
        });
        srow(ui, cx, "progress_indicator", |ui| {
            style::toggle(ui, &mut s.progress_indicator);
        });
    }

    fn page_window(&mut self, ui: &mut Ui, cx: &RowCx) {
        let lang = cx.lang;
        if crate::cli::get().no_tray {
            note(ui, cx, lang.t("Gestartet mit --no-tray: kein Tray-Symbol, „Schließen → Tray“ und „Minimieren → Tray“ wirken bei diesem Start nicht.", "Started with --no-tray: no tray icon, “Close → tray” and “Minimize → tray” have no effect for this start."));
        }
        let s = &mut self.settings;
        srow(ui, cx, "confirm_close", |ui| {
            style::toggle(ui, &mut s.confirm_close);
        });
        srow(ui, cx, "close_to_tray", |ui| {
            style::toggle(ui, &mut s.close_to_tray);
        });
        srow(ui, cx, "minimize_to_tray", |ui| {
            style::toggle(ui, &mut s.minimize_to_tray);
        });
    }

    fn page_integration(&mut self, ui: &mut Ui, cx: &RowCx) {
        let lang = cx.lang;
        if !self.win_probed {
            self.win_autostart = crate::integration::autostart_status();
            self.win_context = crate::integration::context_status();
            self.editor_svc.edit_context = crate::integration::edit_context_status();
            self.win_probed = true;
        }
        if cx.plain() {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(items::GUTTER);
                ui.label(RichText::new(if cfg!(windows) { lang.t("Nur für dich, ohne Administratorrechte.", "Current user only, no admin rights.") } else { lang.t("Nur für dich, Dateien im Benutzerordner.", "Current user only, files in your home folder.") }).small().color(style::dim(ui)));
                if ui.small_button(lang.t("Status aktualisieren", "Refresh status")).clicked() {
                    self.win_probed = false;
                }
            });
            ui.add_space(S1);
        }
        let exe = self.exe.clone();
        let mut err: Option<String> = None;
        // Zeigt der Eintrag auf ein anderes Paneldeck (andere .exe oder Instanz): Hinweis mit dessen Befehl.
        fn other_hint(ui: &mut Ui, lang: crate::state::Lang, status: &crate::integration::Status, what: &str) {
            if let Some(cmd) = status.other() {
                let text = match lang {
                    crate::state::Lang::De => format!("{what} zeigt auf ein anderes Paneldeck:\n{cmd}\n\nEinschalten übernimmt ihn für diese Instanz."),
                    crate::state::Lang::En => format!("{what} points to another Paneldeck:\n{cmd}\n\nTurning it on takes it over for this instance."),
                };
                ui.label(RichText::new(lang.t("→ anderes Paneldeck", "→ another Paneldeck")).small().color(crate::style::ui().warning)).on_hover_text(text);
            }
        }
        // macOS (#150): eigener Hinweis zur Anmeldung (LaunchAgent).
        let mac_hint = cfg!(target_os = "macos").then(|| {
            lang.t(
                "Startet minimiert in der Menüleiste (LaunchAgent in ~/Library/LaunchAgents; macOS zeigt ihn unter Systemeinstellungen → Allgemein → Anmeldeobjekte)",
                "Starts minimized in the menu bar (LaunchAgent in ~/Library/LaunchAgents; macOS lists it under System Settings → General → Login Items)",
            )
        });
        srow_hint(ui, cx, "autostart", mac_hint, |ui| {
            let mut v = self.win_autostart.is_mine();
            if style::toggle(ui, &mut v).changed() {
                match crate::integration::set_autostart(v, &exe) {
                    Ok(()) => self.win_autostart = if v { crate::integration::Status::Mine } else { crate::integration::Status::Off },
                    Err(e) => err = Some(e),
                }
            }
            other_hint(ui, lang, &self.win_autostart, lang.t("Der Autostart-Eintrag", "The autostart entry"));
        });
        if cfg!(target_os = "macos") {
            // macOS (#150): Finder-Integration schreibt Paneldeck nicht – Hinweis auf die Anleitung (Schnellaktion).
            note(ui, cx, lang.t("Finder: „In Paneldeck öffnen“ lässt sich als Schnellaktion selbst anlegen – Anleitung in docs/de/macos.md.", "Finder: “Open in Paneldeck” can be added as a Quick Action yourself – see docs/en/macos.md."));
        } else {
            srow(ui, cx, "context_menu", |ui| {
                let mut v = self.win_context.is_mine();
                if style::toggle(ui, &mut v).changed() {
                    // Die Menütexte landen in der Registry: in der gespeicherten Sprache, nicht in der von `--lang`.
                    let shown = state::lang();
                    state::set_lang(state::persisted_lang(self.settings.language));
                    let r = crate::integration::set_context(v, &exe);
                    state::set_lang(shown);
                    match r {
                        Ok(()) => self.win_context = if v { crate::integration::Status::Mine } else { crate::integration::Status::Off },
                        Err(e) => err = Some(e),
                    }
                }
                other_hint(ui, lang, &self.win_context, lang.t("Das Kontextmenü", "The context menu"));
            });
        }
        // „In Paneldeck-Editor öffnen“ für alle Dateien (#54, nur Windows, Standard aus).
        if let Some(e) = self.edit_context_row(ui, cx, &exe) {
            err = Some(e);
        }
        if let Some(e) = err {
            self.hooks_msg = e;
        }
        if !self.hooks_msg.is_empty() && cx.plain() {
            ui.colored_label(Color32::LIGHT_RED, &self.hooks_msg);
        }
    }

    // ---------------------------------------------------------------------------------------------- Darstellung

    fn page_theme(&mut self, ui: &mut Ui, cx: &RowCx) {
        let lang = cx.lang;
        let top = ui.cursor().min.y;
        srow(ui, cx, "theme", |ui| {
            egui::ComboBox::from_id_salt("themesel").selected_text(self.settings.theme.clone()).show_ui(ui, |ui| {
                for t in &self.themes {
                    ui.selectable_value(&mut self.settings.theme, t.name.clone(), t.name.as_str());
                }
            });
        });
        block(ui, cx, "theme_manage", false, |ui| self.theme_manage_ui(ui));
        tutorial::spot_since(ui, Spot::ThemeManage, top);
        srow(ui, cx, "custom_themes", |ui| {
            if ui.button(lang.t("Neu laden", "Reload")).clicked() {
                self.reload_themes();
            }
            if crate::platform::open::FILE_MANAGER && ui.button(lang.t("Ordner öffnen", "Open folder")).clicked() {
                crate::platform::open::folder(&self.themes_dir());
            }
        });
        link(ui, cx, lang.t("Farben für Sitzungen, Projekte und Tags:", "Colours for sessions, projects and tags:"), Page::AppearancePalettes);
    }

    fn page_fonts(&mut self, ui: &mut Ui, cx: &RowCx) {
        let lang = cx.lang;
        srow(ui, cx, "font_size", |ui| {
            ui.add(egui::Slider::new(&mut self.settings.font_size, 9.0..=28.0).suffix(" pt"));
        });
        srow(ui, cx, "terminal_font", |ui| {
            let builtin = [("Cascadia Mono", lang.t("Cascadia Mono (eingebaut)", "Cascadia Mono (built-in)")), ("Hack", lang.t("Hack (eingebaut)", "Hack (built-in)"))];
            let shown = builtin.iter().find(|(k, _)| *k == self.settings.terminal_font).map(|(_, l)| l.to_string()).unwrap_or_else(|| self.settings.terminal_font.clone());
            egui::ComboBox::from_id_salt("termfont").selected_text(shown).height(360.0).show_ui(ui, |ui| {
                for (k, l) in builtin {
                    ui.selectable_value(&mut self.settings.terminal_font, k.to_string(), l);
                }
                let installed = crate::sysfonts::monospace_families_cached();
                if !installed.is_empty() {
                    ui.separator();
                }
                for f in installed.iter().filter(|f| !builtin.iter().any(|(k, _)| k == f)) {
                    ui.selectable_value(&mut self.settings.terminal_font, f.clone(), f.as_str());
                }
            });
        });
        srow(ui, cx, "ui_scale", |ui| {
            // Erst beim Loslassen übernehmen – sonst skaliert die Oberfläche mitten im Ziehen.
            let mut v = self.scale_pending.unwrap_or(self.settings.ui_scale);
            let r = ui.add(egui::Slider::new(&mut v, 0.7..=2.0).fixed_decimals(2));
            if r.changed() {
                self.scale_pending = Some(v);
            }
            if r.drag_stopped() || (r.changed() && !r.dragged()) {
                self.settings.ui_scale = v;
                self.scale_pending = None;
            }
        });
        link(ui, cx, lang.t("Schrift des Editors:", "Editor font:"), Page::EditorGeneral);
    }

    fn page_transparency(&mut self, ui: &mut Ui, cx: &RowCx) {
        let lang = cx.lang;
        srow(ui, cx, "opacity", |ui| {
            ui.add(egui::Slider::new(&mut self.settings.opacity, 0.3..=1.0).custom_formatter(|v, _| format!("{:.0} %", v * 100.0)));
        });
        let top_rt = ui.cursor().min.y;
        srow(ui, cx, "reduce_transparency", |ui| {
            style::toggle(ui, &mut self.settings.reduce_transparency);
        });
        tutorial::spot_since(ui, Spot::ReduceTransparency, top_rt);
        srow(ui, cx, "animations", |ui| {
            style::toggle(ui, &mut self.settings.animations);
        });
        link(ui, cx, lang.t("Blinkender Cursor:", "Blinking cursor:"), Page::TerminalBehavior);
    }

    // ---------------------------------------------------------------------------------------------- Terminal

    fn page_shell(&mut self, ui: &mut Ui, cx: &RowCx) {
        let lang = cx.lang;
        let cur = self.shells.iter().find(|s| s.id == self.settings.default_shell).map(|s| crate::shells::display_label(s, lang)).unwrap_or_default();
        srow(ui, cx, "default_shell", |ui| {
            egui::ComboBox::from_id_salt("defshell").selected_text(cur).show_ui(ui, |ui| {
                for s in &self.shells {
                    ui.selectable_value(&mut self.settings.default_shell, s.id.clone(), crate::shells::display_label(s, lang));
                }
            });
        });
        srow(ui, cx, "shell_integration", |ui| {
            style::toggle(ui, &mut self.settings.shell_integration);
        });
        // #175: kurzer Hinweis in der Zeile, die Abwägung ausführlich im Tooltip des Schalters.
        srow(ui, cx, "inherit_env", |ui| {
            style::toggle(ui, &mut self.settings.inherit_env).on_hover_text(lang.t(
                "Aus (empfohlen): Neue Terminals lesen die Umgebungsvariablen frisch aus Windows – mit setx oder in den Systemeinstellungen geänderte Variablen gelten sofort, ohne Paneldeck neu zu starten; eine eigene Umgebung der Shell, aus der Paneldeck gestartet wurde, geht dabei für Variablen, die auch in Windows gesetzt sind (z. B. PATH), verloren.\n\nAn: Neue Terminals bekommen genau die Umgebung, mit der Paneldeck gestartet wurde (z. B. aus einer Shell mit eigenem PATH oder eigenen Variablen); in Windows später geänderte Variablen gelten dann erst nach einem Neustart von Paneldeck.\n\nIn beiden Fällen gelten die Variablen des Profils und Paneldecks eigene Variablen. Wirkt für neue Terminals.",
                "Off (recommended): new terminals read the environment variables fresh from Windows – variables changed with setx or in the system settings apply at once, without restarting Paneldeck; a custom environment of the shell Paneldeck was started from is lost for variables that Windows also sets (e.g. PATH).\n\nOn: new terminals get exactly the environment Paneldeck was started with (e.g. from a shell with its own PATH or variables); variables changed in Windows later only apply after restarting Paneldeck.\n\nProfile variables and Paneldeck's own variables apply either way. Applies to new terminals.",
            ));
        });
        link(ui, cx, lang.t("Shell mit eigenem Programm, Startbefehl und Variablen:", "Shell with its own program, startup command and variables:"), Page::TerminalProfiles);
        link(ui, cx, lang.t("Shell-Integration in ssh oder tmux:", "Shell integration in ssh or tmux:"), Page::TerminalRemote);
    }

    fn page_behavior(&mut self, ui: &mut Ui, cx: &RowCx) {
        let lang = cx.lang;
        srow(ui, cx, "scrollback_lines", |ui| {
            ui.add(egui::DragValue::new(&mut self.settings.scrollback_lines).range(1000..=100_000).speed(100.0));
        });
        srow(ui, cx, "cursor_blink", |ui| {
            style::toggle(ui, &mut self.settings.cursor_blink);
        });
        srow(ui, cx, "alt_screen_wheel", |ui| {
            use crate::state::AltScreenWheel;
            let label = |w: AltScreenWheel| match w {
                AltScreenWheel::Arrows => lang.t("Pfeiltasten senden (wie andere Terminals)", "Send arrow keys (like other terminals)"),
                AltScreenWheel::Off => lang.t("Aus", "Off"),
            };
            egui::ComboBox::from_id_salt("altwheel").selected_text(label(self.settings.alt_screen_wheel)).show_ui(ui, |ui| {
                for w in [AltScreenWheel::Arrows, AltScreenWheel::Off] {
                    ui.selectable_value(&mut self.settings.alt_screen_wheel, w, label(w));
                }
            });
        });
        #[cfg(windows)]
        {
            srow(ui, cx, "shift_enter", |ui| {
                style::toggle(ui, &mut self.settings.shift_enter_newline_powershell);
            });
            srow(ui, cx, "bundled_conpty", |ui| {
                style::toggle(ui, &mut self.settings.bundled_conpty);
            });
            if cx.visible("bundled_conpty") && self.settings.bundled_conpty && crate::platform::conpty::bundled_dll_cached().is_none() {
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(items::GUTTER);
                    ui.label(RichText::new(lang.t(
                        "conpty.dll und OpenConsole.exe fehlen neben paneldeck.exe – Sitzungen nutzen den Konsolen-Host von Windows.",
                        "conpty.dll and OpenConsole.exe are missing next to paneldeck.exe – sessions use the console host built into Windows.",
                    )).small().color(crate::style::ui().warning));
                });
                ui.add_space(4.0);
            }
        }
    }

    fn page_clipboard(&mut self, ui: &mut Ui, cx: &RowCx) {
        srow(ui, cx, "osc52_copy", |ui| {
            style::toggle(ui, &mut self.settings.osc52_copy);
        });
        srow(ui, cx, "warn_multiline_paste", |ui| {
            style::toggle(ui, &mut self.settings.warn_multiline_paste);
        });
    }

    /// Profile (Liste und Formular des gewählten Profils).
    fn profiles_ui(&mut self, ui: &mut Ui) {
        let lang = self.settings.language;
        let shells: Vec<(String, String)> = self.shells.iter().map(|s| (s.id.clone(), crate::shells::display_label(s, lang))).collect();
        // WSL-Shells (Kennung, Distribution) für die Shell-Wahl im Profil (#57).
        let wsl_shells: Vec<(String, String)> = self.shells.iter().filter(|s| crate::shells::is_wsl(s) && !crate::shells::is_system_bash(&s.program)).map(|s| (s.id.clone(), crate::shells::wsl_distro_key(s))).collect();
        let theme_names: Vec<String> = self.themes.iter().map(|t| t.name.clone()).collect();
        ui.horizontal(|ui| {
            if ui.button(lang.t("+ Neues Profil", "+ New profile")).clicked() {
                let id = self.next_profile_id();
                self.settings.profiles.push(Profile { id, name: format!("{} {id}", lang.t("Profil", "Profile")), ..Profile::default() });
                self.profile_sel = self.settings.profiles.len() - 1;
            }
            if ui.add_enabled(!self.settings.profiles.is_empty(), egui::Button::new(lang.t("Duplizieren", "Duplicate"))).clicked() {
                if let Some(p) = self.settings.profiles.get(self.profile_sel).cloned() {
                    let id = self.next_profile_id();
                    self.settings.profiles.push(Profile { id, name: format!("{} (2)", p.name), ..p });
                    self.profile_sel = self.settings.profiles.len() - 1;
                }
            }
            if ui.add_enabled(!self.settings.profiles.is_empty(), egui::Button::new(lang.t("Löschen", "Delete"))).clicked() {
                if self.profile_sel < self.settings.profiles.len() {
                    self.settings.profiles.remove(self.profile_sel);
                    self.profile_sel = self.profile_sel.saturating_sub(1);
                }
            }
        });
        ui.add_space(S2);
        if self.settings.profiles.is_empty() {
            ui.label(RichText::new(lang.t("Noch keine Profile. Ein Profil bündelt Shell, Startbefehl, Umgebungsvariablen und Darstellung (z. B. „Produktion“ rot oder „Claude in WSL“).", "No profiles yet. A profile bundles shell, startup command, environment variables and appearance (e.g. “Production” in red or “Claude in WSL”).")).color(style::dim(ui)));
            return;
        }
        ui.horizontal_wrapped(|ui| {
            for (i, p) in self.settings.profiles.iter().enumerate() {
                if afford::choice(ui, self.profile_sel == i, &p.name, 0.0).clicked() {
                    self.profile_sel = i;
                }
            }
        });
        ui.separator();
        let sel = self.profile_sel.min(self.settings.profiles.len() - 1);
        // Ordnerfeld nach der Shell des Profils (#392): WSL-Pfade, Git Bash `/c/x`, `~`.
        let cwd_target = self.folder_target(self.settings.profiles[sel].id);
        let wsl_recent = self.settings.wsl_recent.clone();
        let p = &mut self.settings.profiles[sel];
        prow(ui, lang.t("Name", "Name"), "", |ui| {
            ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(240.0));
        });
        prow(ui, "Shell", lang.t("Leer = Standard-Shell", "Empty = default shell"), |ui| {
            let cur = shells.iter().find(|(i, _)| *i == p.shell_id).map(|(_, l)| l.clone()).unwrap_or_else(|| lang.t("Standard", "Default").to_string());
            egui::ComboBox::from_id_salt("profshell").selected_text(cur).show_ui(ui, |ui| {
                if ui.selectable_label(p.shell_id.is_empty(), lang.t("Standard", "Default")).clicked() {
                    p.shell_id.clear();
                }
                for (i, l) in &shells {
                    if ui.selectable_label(p.shell_id == *i, l).clicked() {
                        p.shell_id = i.clone();
                    }
                }
            });
        });
        if let Some((_, distro)) = wsl_shells.iter().find(|(i, _)| *i == p.shell_id).filter(|_| p.program.trim().is_empty()) {
            // Startet die Abfrage (Login-Shell, /etc/shells), falls noch nichts bekannt ist.
            let login = crate::shellinit::ensure_wsl_login(distro);
            let list = match &login {
                crate::shellinit::Login::Known(i) => Some(i.shells.clone()),
                _ => None,
            };
            let item = |path: &str| match crate::shellinit::Flavor::of_unix_shell(path) {
                Some(_) => path.to_string(),
                None => format!("{path} ({})", lang.t("ohne Integration", "without integration")),
            };
            prow(ui, "Shell in WSL", lang.t("Automatisch = Login-Shell; Auswahl aus /etc/shells", "Automatic = login shell; choice from /etc/shells"), |ui| {
                let auto = format!("{}{}", lang.t("Automatisch", "Automatic"), crate::shellinit::login_suffix(Some(&login), lang));
                let cur = if p.login_shell.is_empty() { auto.clone() } else { item(&p.login_shell) };
                egui::ComboBox::from_id_salt("proflogin").selected_text(cur).show_ui(ui, |ui| {
                    if ui.selectable_label(p.login_shell.is_empty(), &auto).clicked() {
                        p.login_shell.clear();
                    }
                    match &list {
                        Some(l) => {
                            for sh in l {
                                if ui.selectable_label(p.login_shell == *sh, item(sh)).clicked() {
                                    p.login_shell = sh.clone();
                                }
                            }
                        }
                        None => {
                            ui.label(RichText::new(lang.t("/etc/shells wird gelesen …", "Reading /etc/shells …")).small().color(style::dim(ui)));
                        }
                    }
                });
            });
        }
        prow(ui, lang.t("Eigenes Programm", "Custom program"), lang.t("Überschreibt die Shell, z. B. ssh.exe oder wsl.exe", "Overrides the shell, e.g. ssh.exe or wsl.exe"), |ui| {
            ui.add(egui::TextEdit::singleline(&mut p.program).desired_width(240.0));
        });
        prow(ui, lang.t("Argumente", "Arguments"), "", |ui| {
            ui.add(egui::TextEdit::singleline(&mut p.args).desired_width(240.0));
        });
        prow(ui, lang.t("Startordner", "Start folder"), "", |ui| {
            let pid = p.id;
            super::folder_picker::field(ui, ("prof_cwd", pid), &mut p.cwd, cwd_target.clone(), false, lang, &wsl_recent);
        });
        prow(ui, lang.t("Startbefehl", "Startup command"), lang.t("Wird nach dem Start eingegeben (vor `claude`)", "Typed after start (before `claude`)"), |ui| {
            ui.add(egui::TextEdit::singleline(&mut p.startup).desired_width(240.0));
        });
        ui.label(lang.t("Umgebungsvariablen (NAME=WERT, eine pro Zeile)", "Environment variables (NAME=VALUE, one per line)"));
        ui.add(egui::TextEdit::multiline(&mut p.env).desired_rows(3).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace));
        ui.add_space(S1);
        prow(ui, "Theme", "", |ui| {
            let cur = if p.theme.is_empty() { lang.t("Global", "Global").to_string() } else { p.theme.clone() };
            egui::ComboBox::from_id_salt("proftheme").selected_text(cur).show_ui(ui, |ui| {
                if ui.selectable_label(p.theme.is_empty(), lang.t("Global", "Global")).clicked() {
                    p.theme.clear();
                }
                for n in &theme_names {
                    if ui.selectable_label(p.theme == *n, n).clicked() {
                        p.theme = n.clone();
                    }
                }
            });
        });
        prow(ui, lang.t("Schriftgröße", "Font size"), lang.t("0 = global", "0 = global"), |ui| {
            ui.add(egui::DragValue::new(&mut p.font_size).range(0.0..=40.0).speed(0.2));
        });
        prow(ui, lang.t("Farbe", "Color"), lang.t("Farbmarke der Sitzung", "Session color mark"), |ui| {
            ui.vertical(|ui| {
                let cur = p.color.map(crate::palette::hex_of);
                if let Some(h) = palette_ui::pick(ui, ("profile_color", p.id), crate::palette::Area::Hub, cur.as_deref(), palette_ui::PickOpts::HUB, lang) {
                    p.color = Some(crate::palette::rgb(&h));
                }
                if ui.small_button(lang.t("Keine", "None")).clicked() {
                    p.color = None;
                }
            });
        });
        prow(ui, lang.t("Als Administrator", "As administrator"), lang.t("Öffnet ein eigenes Fenster (UAC); nicht eingebettet", "Opens its own window (UAC); not embedded"), |ui| {
            style::toggle(ui, &mut p.admin);
        });
    }

    // ---------------------------------------------------------------------------------------------- Tastenkürzel

    fn page_shortcuts(&mut self, ui: &mut Ui, cx: &RowCx) {
        let lang = cx.lang;
        let ctx = ui.ctx().clone();
        let de = lang == Lang::De;
        note(ui, cx, lang.t("Klicke auf ein Kürzel und drücke die neue Tastenkombination (Esc bricht ab).", "Click a shortcut and press the new key combination (Esc cancels)."));
        ui.add_space(S1);
        let current: Vec<(&str, String)> = keys::DEFAULTS.iter().map(|(id, d)| (*id, self.settings.keys.get(*id).cloned().unwrap_or_else(|| d.to_string()))).collect();
        for (id, cur) in &current {
            let Some(it) = items::item(&format!("key.{id}")) else { continue };
            // Leer = kein Kürzel (z. B. „Alle Sitzungen schließen“): nie „doppelt“.
            // Erlaubte Überdeckung (#394): Strg+Umschalt+D im Editor (alle Vorkommen) und im Terminal Hub (rechts teilen).
            let allowed = |a: &str, b: &str| keys::EDITOR_OVERRIDES.iter().any(|(e, t)| (*e == a && *t == b) || (*e == b && *t == a));
            let dup = !cur.is_empty() && current.iter().any(|(other, c)| other != id && c == cur && !allowed(id, other));
            // Alte Belegung ohne Strg/Alt (vor #256 aufnehmbar): wirkt nicht mehr, darum der Hinweis.
            let plain = keys::parse(cur).is_some_and(|(m, k)| !keys::usable(m, k));
            // Von Hand eingetragenes Zeichen ohne Umschalt (z. B. `Ctrl+?`): löste bis 2.3.1 auch mit Umschalt
            // aus, seit #254 nur noch genau so.
            let shift = keys::parse(cur).is_some_and(|(m, k)| keys::may_need_shift(m, k));
            let capturing = self.capture_key.as_deref() == Some(*id);
            let warn = if dup {
                lang.t("Doppelt belegt!", "Duplicate!")
            } else if plain {
                lang.t("Ohne Strg/Alt – wirkt nicht", "No Ctrl/Alt – inactive")
            } else if shift {
                lang.t(
                    "Ohne Umschalt – wirkt nur, wenn die Tastatur das Zeichen ohne Umschalt liefert; sonst neu aufnehmen",
                    "No Shift – works only if your keyboard types this character without Shift; otherwise record it again",
                )
            } else {
                ""
            };
            srow_hint(ui, cx, it.key, Some(warn), |ui| {
                let txt = if capturing {
                    lang.t("Taste drücken …", "Press keys …").to_string()
                } else if cur.is_empty() {
                    lang.t("(keins)", "(none)").to_string()
                } else {
                    keys::pretty(cur, de)
                };
                let b = egui::Button::new(RichText::new(txt).monospace()).min_size(Vec2::new(190.0, 24.0));
                let b = if capturing { b.fill(style::brand().gamma_multiply(0.35)) } else { b };
                if ui.add(b).clicked() {
                    self.capture_key = Some(id.to_string());
                }
            });
        }
        note(ui, cx, lang.t("Fest: Strg+Umschalt+1…9 Projekt · Alt+1…9 Sitzung · Strg+Alt+Pfeile Bereich", "Fixed: Ctrl+Shift+1…9 project · Alt+1…9 session · Ctrl+Alt+arrows pane"));
        // Aufnahme
        let reject_id = egui::Id::new("key_capture_rejected");
        if let Some(id) = self.capture_key.clone() {
            // Tasten und Text der Aufnahme verbrauchen: sonst schlösse das Esc weiter unten im selben Bild die
            // Einstellungen (statt nur die Aufnahme abzubrechen), und nichts landet woanders (#256).
            let ev: Vec<Event> = ctx.input_mut(|i| {
                let (taken, rest): (Vec<Event>, Vec<Event>) = std::mem::take(&mut i.events).into_iter().partition(|e| matches!(e, Event::Key { .. } | Event::Text(_)));
                i.events = rest;
                taken
            });
            for e in ev {
                if let Event::Key { key, pressed: true, modifiers, .. } = e {
                    if key == Key::Escape {
                        self.capture_key = None;
                    } else if keys::usable(modifiers, key) {
                        self.settings.keys.insert(id.clone(), keys::format(modifiers, key));
                        self.capture_key = None;
                    } else {
                        // Ohne Strg/Alt: nicht übernehmen, weiter aufnehmen und erklären.
                        ctx.data_mut(|d| d.insert_temp(reject_id, keys::pretty(&keys::format(modifiers, key), de)));
                    }
                }
            }
            if let Some(k) = ctx.data(|d| d.get_temp::<String>(reject_id)).filter(|_| self.capture_key.is_some()) {
                ui.label(RichText::new(match lang {
                    Lang::De => format!("„{k}“ geht nicht ohne Strg oder Alt – es finge jede solche Eingabe in den Terminals ab (F-Tasten gehen auch allein)."),
                    Lang::En => format!("“{k}” needs Ctrl or Alt – on its own it would catch every such keystroke in the terminals (F keys also work alone)."),
                }).small().color(Color32::LIGHT_RED));
            }
        }
        if self.capture_key.is_none() {
            ctx.data_mut(|d| d.remove::<String>(reject_id));
        }
    }

    // ---------------------------------------------------------------------------------------------- Claude

    fn page_claude_general(&mut self, ui: &mut Ui, cx: &RowCx) {
        let top = ui.cursor().min.y;
        srow(ui, cx, "claude_mode", |ui| {
            style::toggle(ui, &mut self.settings.claude_mode);
        });
        tutorial::spot_since(ui, Spot::ClaudeMode, top);
        srow(ui, cx, "claude_resume", |ui| {
            style::toggle(ui, &mut self.settings.claude_resume);
        });
        link(ui, cx, cx.lang.t("Standards für neue Claude-Sitzungen:", "Defaults for new Claude sessions:"), Page::ClaudeSession);
    }

    /// Hinweis, solange der Claude-Modus aus ist (die Einstellungen gelten erst mit ihm).
    fn claude_off_note(&self, ui: &mut Ui, cx: &RowCx) {
        if !self.settings.claude_mode {
            link(ui, cx, cx.lang.t("Der Claude-Modus ist aus – diese Einstellungen wirken erst, wenn er an ist.", "Claude mode is off – these settings only take effect once it is on."), Page::ClaudeGeneral);
            ui.add_space(S1);
        }
    }

    /// Standards neuer Claude-Sitzungen: eine Seite je früherem Reiter (*Claude*, *Zugriff*, *Erweitert*, #292).
    fn page_claude_defaults(&mut self, ui: &mut Ui, cx: &RowCx, tab: opts_tabs::Tab) {
        let lang = cx.lang;
        self.claude_off_note(ui, cx);
        let key = match tab {
            opts_tabs::Tab::Claude => "claude_defaults_claude",
            opts_tabs::Tab::Access => "claude_defaults_access",
            _ => "claude_defaults_advanced",
        };
        if tab == opts_tabs::Tab::Claude {
            note(ui, cx, lang.t(
                "Gelten für Sitzungen aus dem Dialog „Neue Sitzung“ (Vorauswahl), dem Kontextmenü, Vorlagen und über MCP, wenn nichts angegeben ist. „Standard von Claude Code“ setzt keine Option. bypassPermissions lässt sich nur im Dialog „Neue Sitzung“ für eine einzelne Sitzung wählen. Ohne Permission-Mode starten Agenten-Sitzungen mit „default“.",
                "Apply to sessions from the “New session” dialog (preselection), the context menu, templates and via MCP when nothing is given. “Claude Code default” sets no option. bypassPermissions can only be chosen in the “New session” dialog for a single session. Without a permission mode, agent sessions start with “default”.",
            ));
            if self.settings.claude_bypass_dropped && cx.plain() {
                ui.label(RichText::new(lang.t(
                    "Hinweis: bypassPermissions stand als Standard in settings.json und wurde entfernt – es ist nur noch im Dialog „Neue Sitzung“ für eine einzelne Sitzung wählbar.",
                    "Note: bypassPermissions was set as a default in settings.json and has been removed – it can now only be chosen in the “New session” dialog for a single session.",
                )).small().color(crate::style::ui().warning));
            }
            ui.add_space(S1);
        }
        let top = ui.cursor().min.y;
        let before = self.settings.claude_defaults.clone();
        // Ungültige Werte bleiben nur im Eingabefeld; gespeichert und angewendet werden nur gültige Werte. Standards gelten
        // für jede Shell: Windows-Dialoge; Linux-Pfade bleiben möglich und gelten nur in WSL-Sitzungen.
        block(ui, cx, key, false, |ui| {
            let target = super::folder_picker::Target::Windows;
            let none = ClaudeOpts::default();
            let chk = super::dialogs::check_opts(ui.ctx(), lang, &self.settings.claude_defaults, "defaults", super::dialogs::OptsPlace::Defaults, &target);
            super::dialogs::claude_tab(ui, lang, &mut self.settings.claude_defaults, "defaults", super::dialogs::OptsPlace::Defaults, &target, &none, &chk, tab, |_| {}, |_| {});
        });
        if self.settings.claude_defaults != before {
            // Neue Standards gelten auch im Dialog wieder (statt der zuletzt dort gewählten Optionen).
            self.settings.last_claude_opts = None;
        }
        if tab == opts_tabs::Tab::Claude {
            tutorial::spot_since(ui, Spot::ClaudeDefaults, top);
        }
    }

    fn page_claude_costs(&mut self, ui: &mut Ui, cx: &RowCx) {
        let lang = cx.lang;
        self.claude_off_note(ui, cx);
        let top = ui.cursor().min.y;
        srow(ui, cx, "daily_budget", |ui| {
            ui.add(egui::DragValue::new(&mut self.settings.daily_budget).range(0.0..=100000.0).speed(0.5).prefix("$ ").fixed_decimals(2));
        });
        srow(ui, cx, "session_budget", |ui| {
            ui.add(egui::DragValue::new(&mut self.settings.session_budget).range(0.0..=100000.0).speed(0.5).prefix("$ ").fixed_decimals(2));
        });
        tutorial::spot_since(ui, Spot::ClaudeBudget, top);
        ui.add_space(S2);
        block(ui, cx, "prices", true, |ui| {
            egui::Grid::new("prices").num_columns(3).spacing([14.0, 6.0]).show(ui, |ui| {
                ui.label("");
                ui.label(RichText::new("Input").small().color(style::dim(ui)));
                ui.label(RichText::new("Output").small().color(style::dim(ui)));
                ui.end_row();
                for (name, p) in [("Opus", &mut self.settings.prices.opus), ("Sonnet", &mut self.settings.prices.sonnet), ("Haiku", &mut self.settings.prices.haiku)] {
                    ui.label(name);
                    ui.add(egui::DragValue::new(&mut p[0]).range(0.0..=1000.0).speed(0.05).prefix("$ ").fixed_decimals(2));
                    ui.add(egui::DragValue::new(&mut p[1]).range(0.0..=1000.0).speed(0.05).prefix("$ ").fixed_decimals(2));
                    ui.end_row();
                }
                ui.label(lang.t("Cache lesen", "Cache read"));
                ui.add(egui::DragValue::new(&mut self.settings.prices.cache_read).range(0.0..=5.0).speed(0.01).prefix("× ").fixed_decimals(2));
                ui.label("");
                ui.end_row();
                ui.label(lang.t("Cache schreiben", "Cache write"));
                ui.add(egui::DragValue::new(&mut self.settings.prices.cache_write).range(0.0..=5.0).speed(0.01).prefix("× ").fixed_decimals(2));
                ui.label("");
                ui.end_row();
            });
        });
    }

    fn page_claude_hooks(&mut self, ui: &mut Ui, cx: &RowCx) {
        let lang = cx.lang;
        self.claude_off_note(ui, cx);
        block(ui, cx, "hooks_global", true, |ui| {
            let mut install = false;
            ui.horizontal(|ui| {
                if self.hooks_global {
                    ui.colored_label(crate::style::ui().success, lang.t("global installiert", "installed globally"));
                    if ui.button(lang.t("Entfernen", "Remove")).clicked() {
                        self.hooks_msg = match hooks::global_settings_path().map(|p| hooks::uninstall_global(&p)) {
                            Some(Ok(())) => String::new(),
                            Some(Err(e)) => e,
                            None => state::t("~/.claude nicht gefunden", "~/.claude not found").into(),
                        };
                        self.refresh_hooks_global();
                    }
                } else if ui.button(lang.t("In ~/.claude/settings.json installieren", "Install into ~/.claude/settings.json")).clicked() {
                    install = true;
                }
            });
            // Global eingetragen, aber für eine andere paneldeck.exe (#260): Meldungen ohne Schlüssel zählen nur
            // für den Status.
            if let Some(other) = self.hooks_foreign.clone() {
                ui.colored_label(
                    Color32::from_rgb(0xe0, 0xa0, 0x40),
                    format!("{} {}", lang.t("⚠ Die eingetragenen Hooks zeigen auf eine andere paneldeck.exe:", "⚠ The installed hooks point to another paneldeck.exe:"), crate::mcp::short(&other, 80)),
                );
                ui.label(RichText::new(lang.t(
                    "Ihre Meldungen zählen nur für den Status (keine Zuordnung der Konversation). Neu eintragen, damit sie auf dieses Programm zeigen.",
                    "Their messages only count for the status (no conversation tracking). Reinstall so they point to this program.",
                )).small().color(style::dim(ui)));
                if ui.button(lang.t("Neu eintragen", "Reinstall")).clicked() {
                    install = true;
                }
            }
            if install {
                self.hooks_msg = match hooks::global_settings_path().map(|p| hooks::install_global(&p, &self.exe)) {
                    Some(Ok(())) => String::new(),
                    Some(Err(e)) => e,
                    None => state::t("~/.claude nicht gefunden", "~/.claude not found").into(),
                };
                self.refresh_hooks_global();
            }
            if !self.hooks_msg.is_empty() {
                ui.colored_label(Color32::LIGHT_RED, &self.hooks_msg);
            }
        });
    }

    // ---------------------------------------------------------------------------------------------- Daten, Info

    fn page_diagnostics(&mut self, ui: &mut Ui, cx: &RowCx) {
        let lang = cx.lang;
        let ctx = ui.ctx().clone();
        srow(ui, cx, "diagnostics", |ui| {
            if ui.button(lang.t("Diagnose kopieren", "Copy diagnostics")).clicked() {
                ctx.copy_text(self.diagnostics());
            }
            if ui.button(lang.t("Fehlerbericht kopieren", "Copy error report")).on_hover_text(lang.t("Diagnose und Ende des Absturzprotokolls in einem Schritt", "Diagnostics plus the end of the crash log in one step")).clicked() {
                ctx.copy_text(self.error_report());
            }
        });
        let dir = self.dir.clone();
        srow_hint(ui, cx, "data_folder", Some(&dir.display().to_string()), |ui| {
            if crate::platform::open::FILE_MANAGER && ui.button(lang.t("Datenordner öffnen", "Open data folder")).clicked() {
                crate::platform::open::folder(&dir);
            }
        });
        link(ui, cx, lang.t("Programm, Profil und Startoptionen:", "Program, profile and start overrides:"), Page::AboutVersion);
    }

    fn page_about(&mut self, ui: &mut Ui, cx: &RowCx) {
        let lang = cx.lang;
        block(ui, cx, "about_info", false, |ui| {
            ui.label(RichText::new(format!("Paneldeck {}", env!("CARGO_PKG_VERSION"))).heading());
            // macOS (#150): experimentell, unsigniert – Hinweis wie im Release-Text.
            if cfg!(target_os = "macos") {
                ui.label(RichText::new(lang.t(
                    "macOS-Version: experimentell und nicht signiert. Fehler und Eindrücke bitte als Issue auf GitHub melden.",
                    "macOS version: experimental and not signed. Please report problems and feedback as an issue on GitHub.",
                )).color(Color32::from_rgb(0xe5, 0xc0, 0x7b)));
            }
            ui.label(RichText::new(lang.t("Terminal-Zentrale für Claude Code und viele Terminals – funktioniert offline; kontaktiert GitHub nur, wenn du nach Updates suchst oder die automatische Prüfung einschaltest.", "Terminal hub for Claude Code and many terminals – works offline; only contacts GitHub when you check for updates or switch on the automatic check.")).color(style::dim(ui)));
            ui.add_space(S2);
            let top = ui.cursor().min.y;
            for (k, v) in [
                (lang.t("Programm", "Program"), self.exe.display().to_string()),
                (lang.t("Datenordner", "Data folder"), self.dir.display().to_string()),
                (lang.t("Herkunft", "Source"), crate::paths::source_label(crate::paths::get().source, lang).to_string()),
                (lang.t("Profil", "Profile"), crate::paths::get().profile.clone().unwrap_or_else(|| lang.t("keins (Standard)", "none (default)").to_string())),
                (lang.t("Startoptionen", "Start overrides"), {
                    let o = crate::cli::get().overrides();
                    if o.is_empty() { lang.t("keine", "none").to_string() } else { o.join("  ") }
                }),
                ("Build", format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)),
            ] {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{k}:")).color(style::dim(ui)));
                    ui.label(RichText::new(v).small());
                });
            }
            tutorial::spot_since(ui, Spot::AboutInfo, top);
        });
        link(ui, cx, lang.t("Diagnose und Fehlerbericht kopieren:", "Copy diagnostics and error report:"), Page::DataDiagnostics);
        link(ui, cx, lang.t("Updates:", "Updates:"), Page::AboutUpdates);
        link(ui, cx, lang.t("Lizenzen:", "Licenses:"), Page::AboutLicenses);
    }

    pub(super) fn editor_ui(&mut self, ctx: &egui::Context) {
        let lang = self.settings.language;
        let Some(ed) = self.editor.as_mut() else { return };
        let mut open = true;
        let save_key = ctx.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::S));
        style::movable(ctx, Id::new("claude_files_win"), egui::Window::new(lang.t("Claude-Dateien", "Claude files")))
            .open(&mut open)
            .default_size([860.0, 520.0])
            .show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(250.0);
                        for i in 0..ed.files.len() {
                            let exists = ed.files[i].path.exists();
                            let dirty = if i == ed.sel { ed.dirty() } else { ed.drafts.get(&i).is_some_and(|(t, o, _)| t != o) };
                            let mut text = RichText::new(format!("{}{}", ed.files[i].label, if dirty { " •" } else { "" }));
                            if !exists {
                                text = text.color(style::dim(ui));
                            }
                            if ui.selectable_label(ed.sel == i, text).on_hover_text(ed.files[i].path.display().to_string()).clicked() {
                                ed.select(i);
                            }
                        }
                        ui.add_space(S2);
                        ui.label(RichText::new(lang.t("Grau = Datei existiert noch nicht und wird beim Speichern angelegt.", "Grey = file does not exist yet and is created on save.")).small().color(style::dim(ui)));
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        ui.label(RichText::new(ed.files[ed.sel].path.display().to_string()).small().color(style::dim(ui)));
                        egui::ScrollArea::vertical().max_height(400.0).show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut ed.text)
                                    .font(egui::TextStyle::Monospace)
                                    .code_editor()
                                    .desired_rows(22)
                                    .desired_width(f32::INFINITY),
                            );
                        });
                        ui.horizontal(|ui| {
                            if ui.add_enabled(ed.dirty() && !ed.readonly, egui::Button::new(lang.t("Speichern (Strg+S)", "Save (Ctrl+S)"))).clicked() || (save_key && ed.dirty() && !ed.readonly) {
                                ed.save();
                            }
                            if ui.button(lang.t("Neu laden", "Reload")).clicked() {
                                ed.load();
                                ed.msg.clear();
                            }
                            if !ed.msg.is_empty() {
                                let col = if ed.msg_ok { crate::style::ui().success } else { Color32::LIGHT_RED };
                                ui.colored_label(col, &ed.msg);
                            } else if ed.dirty() {
                                ui.label(RichText::new(lang.t("Ungespeicherte Änderungen", "Unsaved changes")).color(style::dim(ui)));
                            }
                        });
                    });
                });
            });
        if !open {
            self.editor = None;
        }
    }

    pub(super) fn apply_font(&mut self, ctx: &egui::Context) {
        // Terminal- und Editorschrift (#54) zusammen: eine Änderung an einer baut die Schriften neu.
        let want = format!("{}\u{1}{}", self.settings.terminal_font, self.settings.editor.font);
        if self.applied_font != want {
            setup_fonts(ctx, &self.settings.terminal_font, &self.settings.editor.font);
            self.applied_font = want;
        }
    }
}

/// „N Treffer auf M Seiten“.
fn hits_text(lang: Lang, n: usize, pages: usize) -> String {
    match lang {
        Lang::De => format!("{n} {} auf {pages} {}", if n == 1 { "Treffer" } else { "Treffer" }, if pages == 1 { "Seite" } else { "Seiten" }),
        Lang::En => format!("{n} {} on {pages} {}", if n == 1 { "hit" } else { "hits" }, if pages == 1 { "page" } else { "pages" }),
    }
}

/// Zeile eines Profils (Formular ohne eigene Einträge der Suche): Beschriftung links, Bedienelement rechts.
fn prow(ui: &mut Ui, label: &str, hint: &str, add: impl FnOnce(&mut Ui)) {
    let w = ui.available_width();
    let lw = items::label_width(w);
    ui.allocate_ui_with_layout(Vec2::new(w, 42.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        let x0 = ui.cursor().min.x;
        ui.allocate_ui_with_layout(Vec2::new(lw, 42.0), egui::Layout::top_down(egui::Align::LEFT).with_main_align(egui::Align::Center), |ui| {
            ui.label(label);
            if !hint.is_empty() {
                ui.label(RichText::new(hint).small().color(style::dim(ui)));
            }
        });
        let used = ui.cursor().min.x - x0;
        ui.add_space((lw + S3 - used).max(0.0));
        add(ui);
    });
    ui.add_space(4.0);
}

/// Terminal › Andere Rechner: Schnipsel für die Shell-Integration auf entfernten Rechnern (ssh) und in eigenen
/// Unter-Shells, je Shell zum Kopieren (#57).
fn terminal_tab(ui: &mut Ui, lang: Lang) {
    ui.label(RichText::new(lang.t("Shell-Integration auf anderen Rechnern", "Shell integration on other machines")).strong());
    ui.label(RichText::new(lang.t(
        "In einer verschachtelten Shell (ssh, tmux, eine Unter-Shell) kommen keine Befehlsmarken von Paneldeck an; die Seitenleiste zeigt dann „ohne Shell-Integration“, und run_command liefert nur eine ungefähre Ausgabe. Mit diesen Zeilen in der Startdatei der Shell auf dem anderen Rechner meldet sie Prompt, Befehlsstart und Exit-Code wieder (ohne Ordner).",
        "In a nested shell (ssh, tmux, a sub-shell) no command marks reach Paneldeck; the sidebar then shows “without shell integration” and run_command only returns approximate output. With these lines in the shell's startup file on the other machine it reports prompt, command start and exit code again (without the folder).",
    )).small().color(style::dim(ui)));
    ui.add_space(S2);
    for (name, file, text) in [
        ("bash", "~/.bashrc", crate::shellinit::REMOTE_BASH),
        ("zsh", "~/.zshrc", crate::shellinit::REMOTE_ZSH),
        ("fish", "~/.config/fish/conf.d/paneldeck.fish", crate::shellinit::REMOTE_FISH),
    ] {
        ui.horizontal(|ui| {
            ui.label(RichText::new(name).strong());
            ui.label(RichText::new(file).small().monospace().color(style::dim(ui)));
            if ui.small_button(lang.t("Kopieren", "Copy")).clicked() {
                ui.ctx().copy_text(text.to_string());
            }
        });
        let mut t = text.to_string();
        ui.add(egui::TextEdit::multiline(&mut t).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY).desired_rows(text.lines().count()).interactive(false));
        ui.add_space(S1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_counts_read_naturally_in_both_languages() {
        assert_eq!(hits_text(Lang::De, 4, 3), "4 Treffer auf 3 Seiten");
        assert_eq!(hits_text(Lang::En, 4, 3), "4 hits on 3 pages");
        assert_eq!(hits_text(Lang::En, 1, 1), "1 hit on 1 page");
        assert_eq!(hits_text(Lang::De, 1, 1), "1 Treffer auf 1 Seite");
    }
}
