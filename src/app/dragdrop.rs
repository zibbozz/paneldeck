//! Ziehen und Ablegen, Tray, abgedockte Fenster.

use super::*;

impl App {
    /// Position der Ablage-Linie im Baum: (y, x links, x rechts) – dort, wo das Element einsortiert würde.
    pub(super) fn drop_line(&self, target: &str, folder_drag: Option<&str>, dragged: u64) -> Option<(f32, f32, f32)> {
        let g = &self.tree_geo;
        let x0 = *g.x.get(target)?;
        let full = g.full.get(target)?;
        let head = g.head.get(target)?;
        if !target.is_empty() && (full.max.y - head.max.y) < 1.0 {
            return None; // eingeklappt
        }
        let end = if target.is_empty() { full.max.y - 1.0 } else { full.max.y - 5.0 };
        let proj = self.projects.iter().find(|p| p.id == self.active_project)?;
        let mut sess: Vec<u64> = self.entries.iter().filter(|(i, e)| e.meta.project == self.active_project && self.in_win(e) && e.meta.group == target && **i != dragged).map(|(i, _)| *i).collect();
        sess.sort();
        let nsubs = proj.folders.iter().filter(|f| state::folder_parent(&f.path) == target && Some(f.path.as_str()) != folder_drag).count();
        if sess.is_empty() && nsubs == 0 {
            return None; // leerer Ordner: keine Linie
        }
        let first_sess_y = sess.first().and_then(|i| g.sess.get(i)).map(|r| r.min.y - 1.0);
        let y = match folder_drag {
            Some(d) => {
                let dl = state::folder_leaf(d).to_lowercase();
                let mut subs: Vec<&state::Folder> = proj.folders.iter().filter(|f| state::folder_parent(&f.path) == target && f.path != d).collect();
                subs.sort_by_key(|f| state::folder_leaf(&f.path).to_lowercase());
                match subs.iter().find(|f| state::folder_leaf(&f.path).to_lowercase() > dl).and_then(|f| g.head.get(&f.path)) {
                    Some(h) => h.min.y - 1.0,
                    None => first_sess_y.unwrap_or(end),
                }
            }
            None => sess.iter().find(|i| **i > dragged).and_then(|i| g.sess.get(i)).map(|r| r.min.y - 1.0).unwrap_or(end),
        };
        // Innerhalb der Markierung bleiben (nicht auf/unter dem Rahmen)
        let y = if target.is_empty() { y } else { y.clamp(head.max.y + 2.0, full.max.y - 5.0) };
        Some((y, x0, if target.is_empty() { g.right } else { g.right.min(full.max.x) }))
    }

    /// Sitzungen per Drag & Drop in Bereiche ziehen (Mitte = ersetzen/tauschen, Rand = teilen).
    pub(super) fn handle_drag(&mut self, ctx: &egui::Context) {
        // Eine Vorlage aus der Bibliothek (#244) hat eigene Ziele. Die Fläche der Sitzungsliste gilt nur für dieses Bild.
        let list_rect = self.tpl.list_rect.take();
        if self.tpl.dragging.is_some() {
            self.handle_tpl_drag(ctx, list_rect);
            return;
        }
        let folder_drag = self.dragging_folder.clone();
        let dragged = match (&folder_drag, self.dragging) {
            (Some(_), _) => u64::MAX,
            (None, Some(d)) => d,
            _ => return,
        };
        let (pos, released, esc) = ctx.input(|i| (i.pointer.hover_pos().or(i.pointer.latest_pos()), i.pointer.any_released(), i.key_pressed(Key::Escape)));
        let name = match &folder_drag {
            Some(f) => format!("📁 {} ({})", state::folder_leaf(f), self.folder_sessions(f).len()),
            None => self.entries.get(&dragged).map(|e| e.display_name()).unwrap_or_default(),
        };
        // Sitzung eines anderen Projekts (gefilterte Seitenleiste): nie in Ordner oder Layout des gezeigten Projekts (#258).
        let foreign = folder_drag.is_none() && self.entries.get(&dragged).is_some_and(|e| e.meta.project != self.active_project);
        // Ordner in der Seitenleiste als Ablageziel: der kleinste Bereich unter dem Zeiger gewinnt.
        let folder_target = pos.filter(|_| !foreign).and_then(|p| {
            self.folder_rects.iter().filter(|(f, r)| r.contains(p) && folder_drag.as_ref().is_none_or(|d| !state::folder_within(f, d))).min_by(|a, b| (a.1.width() * a.1.height()).total_cmp(&(b.1.width() * b.1.height()))).map(|(f, r)| (f.clone(), *r))
        });
        if let Some((fpath, r)) = folder_target {
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::new("drag_layer")));
            let accent = crate::style::brand();
            let is_root = fpath.is_empty();
            if !is_root {
                painter.rect_filled(r, 6.0, Color32::from_rgba_unmultiplied(0xd9, 0x77, 0x57, 40));
                painter.rect_stroke(r.shrink(1.0), 6.0, Stroke::new(1.5_f32, accent), StrokeKind::Inside);
            }
            if let Some((y, x0, x1)) = self.drop_line(&fpath, folder_drag.as_deref(), dragged) {
                painter.line_segment([Pos2::new(x0 + 4.0, y), Pos2::new(x1 - 4.0, y)], Stroke::new(2.5_f32, accent));
                painter.circle_filled(Pos2::new(x0 + 4.0, y), 4.0, accent);
            }
            if let Some(p) = pos {
                let lang = self.settings.language;
                let label = if is_root { lang.t("Kein Ordner", "No folder").to_string() } else { format!("→ {fpath}") };
                let g = painter.layout_no_wrap(format!("{name}  {label}"), FontId::proportional(13.0), Color32::WHITE);
                let mut lr = Rect::from_min_size(p + Vec2::new(12.0, 12.0), g.size() + Vec2::new(12.0, 6.0));
                let sr = ctx.content_rect();
                if lr.max.x > sr.max.x - 4.0 {
                    lr = lr.translate(Vec2::new(sr.max.x - 4.0 - lr.max.x, 0.0));
                }
                painter.rect_filled(lr, 4.0, Color32::from_black_alpha(200));
                painter.galley(lr.min + Vec2::new(6.0, 3.0), g, Color32::WHITE);
                ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            }
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                self.dragging = None;
                self.dragging_folder = None;
            } else if ctx.input(|i| i.pointer.any_released()) {
                self.dragging = None;
                self.dragging_folder = None;
                match folder_drag {
                    Some(fd) => self.apply(ctx, Action::MoveFolder(fd, fpath)),
                    None => self.set_group(dragged, fpath),
                }
            }
            return;
        }
        let target = pos.and_then(|p| self.pane_rects.iter().find(|(_, r)| r.contains(p)).map(|(id, r)| (*id, *r, p)));

        let zone = drop_zone;
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::new("drag_layer")));
        let accent = crate::style::brand();
        // Ablage-Symbole an allen Rändern der Bereiche (das aktive Ziel hervorgehoben).
        {
            let lang = self.settings.language;
            let active = target.map(|(tid, r, p)| (tid, zone(r, p)));
            for (pid, r) in self.pane_rects.iter() {
                if *pid == dragged {
                    continue;
                }
                let spots = [
                    (0u8, r.center(), lang.t("Ersetzen / tauschen", "Replace / swap")),
                    (1, Pos2::new(r.min.x + r.width() * 0.125, r.center().y), lang.t("Links teilen", "Split left")),
                    (2, Pos2::new(r.max.x - r.width() * 0.125, r.center().y), lang.t("Rechts teilen", "Split right")),
                    (3, Pos2::new(r.center().x, r.min.y + r.height() * 0.125), lang.t("Oben teilen", "Split above")),
                    (4, Pos2::new(r.center().x, r.max.y - r.height() * 0.125), lang.t("Unten teilen", "Split below")),
                ];
                for (z, c, label) in spots {
                    let on = active == Some((*pid, z));
                    let (rad, a) = if on { (17.0_f32, 255u8) } else { (12.0, 130) };
                    painter.circle_filled(c, rad, Color32::from_rgba_unmultiplied(0xd9, 0x77, 0x57, if on { 255 } else { 110 }));
                    painter.circle_stroke(c, rad, Stroke::new(1.0_f32, Color32::from_black_alpha(a / 2)));
                    let w = Color32::from_rgba_unmultiplied(255, 255, 255, a);
                    let k = rad * 0.38;
                    let pts = match z {
                        1 => vec![Pos2::new(c.x - k, c.y), Pos2::new(c.x + k, c.y - k), Pos2::new(c.x + k, c.y + k)],
                        2 => vec![Pos2::new(c.x + k, c.y), Pos2::new(c.x - k, c.y - k), Pos2::new(c.x - k, c.y + k)],
                        3 => vec![Pos2::new(c.x, c.y - k), Pos2::new(c.x - k, c.y + k), Pos2::new(c.x + k, c.y + k)],
                        4 => vec![Pos2::new(c.x, c.y + k), Pos2::new(c.x - k, c.y - k), Pos2::new(c.x + k, c.y - k)],
                        _ => vec![],
                    };
                    if pts.is_empty() {
                        painter.rect_stroke(Rect::from_center_size(c, Vec2::splat(k * 1.8)), 2.0, Stroke::new(1.6_f32, w), StrokeKind::Inside);
                    } else {
                        painter.add(egui::Shape::convex_polygon(pts, w, Stroke::NONE));
                    }
                    if on {
                        let g = painter.layout_no_wrap(label.to_string(), FontId::proportional(12.5), Color32::WHITE);
                        let lr = Rect::from_center_size(Pos2::new(c.x, c.y + rad + 14.0), g.size() + Vec2::new(14.0, 6.0));
                        let lr = lr.translate(Vec2::new((r.min.x + 4.0 - lr.min.x).max(0.0) - (lr.max.x - r.max.x + 4.0).max(0.0), 0.0));
                        painter.rect_filled(lr, 4.0, Color32::from_black_alpha(215));
                        painter.galley(lr.center() - g.size() / 2.0, g, Color32::WHITE);
                    }
                }
            }
        }
        if let Some((tid, r, p)) = target {
            if tid != dragged {
                let z = zone(r, p);
                let hl = match z {
                    1 => Rect::from_min_max(r.min, Pos2::new(r.center().x, r.max.y)),
                    2 => Rect::from_min_max(Pos2::new(r.center().x, r.min.y), r.max),
                    3 => Rect::from_min_max(r.min, Pos2::new(r.max.x, r.center().y)),
                    4 => Rect::from_min_max(Pos2::new(r.min.x, r.center().y), r.max),
                    _ => r,
                };
                painter.rect_filled(hl, 0.0, Color32::from_rgba_unmultiplied(0xd9, 0x77, 0x57, 70));
                painter.rect_stroke(hl.shrink(2.0), 4.0, Stroke::new(2.0_f32, accent), StrokeKind::Inside);
            }
        }
        if let Some(p) = pos {
            let g = painter.layout_no_wrap(name, FontId::proportional(13.0), Color32::WHITE);
            let mut r = Rect::from_min_size(p + Vec2::new(12.0, 12.0), g.size() + Vec2::new(12.0, 6.0));
            let sr = ctx.content_rect();
            if r.max.x > sr.max.x - 4.0 {
                r = r.translate(Vec2::new(sr.max.x - 4.0 - r.max.x, 0.0));
            }
            painter.rect_filled(r, 4.0, Color32::from_black_alpha(200));
            painter.galley(r.min + Vec2::new(6.0, 3.0), g, Color32::WHITE);
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        if esc {
            self.dragging = None;
            self.dragging_folder = None;
            return;
        }
        if !released {
            return;
        }
        self.dragging = None;
        self.dragging_folder = None;
        let Some((tid, r, p)) = target else { return };
        if tid == dragged || self.layout.is_none() {
            return;
        }
        let z = zone(r, p);
        if let Some(fd) = folder_drag {
            self.open_folder_at(&fd, tid, z);
            return;
        }
        if foreign {
            // Wie ein Klick in der Seitenleiste: in ihr Projekt wechseln und sie dort zeigen.
            self.apply(ctx, Action::Activate(dragged));
            return;
        }
        self.drop_into_pane(dragged, tid, z);
    }

    /// Sitzung `dragged` in den Bereich `tid` legen (Zone `z` wie [`drop_zone`]): ersetzen/tauschen oder teilen.
    pub(super) fn drop_into_pane(&mut self, dragged: u64, tid: u64, z: u8) {
        // Eine abgedockte Sitzung, die ins Hauptfenster gezogen wird, wird wieder angedockt.
        self.detached.retain(|d| *d != dragged);
        let Some(mut layout) = self.layout.take() else { return };
        let visible = layout.contains(dragged);
        if z == 0 {
            if visible {
                layout.swap_leaves(dragged, tid);
            } else {
                layout.replace_leaf(tid, dragged);
            }
            self.layout = Some(layout);
        } else {
            if visible {
                layout = layout.remove_leaf(dragged).unwrap_or(Node::Leaf(tid));
            }
            let (dir, before) = match z {
                1 => (Dir::Horizontal, true),
                2 => (Dir::Horizontal, false),
                3 => (Dir::Vertical, true),
                _ => (Dir::Vertical, false),
            };
            layout.split_leaf_at(tid, dragged, dir, before);
            self.layout = Some(layout);
        }
        self.focused = dragged;
        self.want_focus = Some(dragged);
        if let Some(e) = self.entries.get_mut(&dragged) {
            e.attention = false;
        }
    }

    pub(super) fn handle_drops(&mut self, ctx: &egui::Context) {
        // Nur im Terminal Hub: die Bereichsflächen (`pane_rects`) stammen vom letzten Zeichnen dort – in der Todo-Liste
        // landete der Pfad sonst in einem unsichtbaren Terminal und das Layout änderte sich (#259).
        // Die Ablage-Hervorhebung wird je Bild neu gezeichnet (kein Zustand, der stehen bliebe); die abgelegten Dateien
        // bleiben unangetastet in der Eingabe (hier wird nur gelesen), andere Empfänger sehen sie weiter. Die Flächen der
        // Bereiche gelten in der Todo-Liste nicht: leeren, damit nichts (Ablage-Ziele beim Ziehen, Screenshot einer
        // Sitzung über MCP) mit Flächen des unsichtbaren Terminal Hub rechnet.
        if self.mode != titlebar::Mode::Terminals {
            self.pane_rects.clear();
            return;
        }
        let (files, hover, hovering) = ctx.input(|i| (i.raw.dropped_files.clone(), i.pointer.hover_pos(), !i.raw.hovered_files.is_empty()));
        let target = hover.and_then(|p| self.pane_rects.iter().find(|(_, r)| r.contains(p)).map(|(id, r)| (*id, *r)));
        if hovering {
            if let Some((_, r)) = target {
                let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::new("drop_hint")));
                painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(0xd9, 0x77, 0x57, 60));
                painter.rect_stroke(r.shrink(2.0), 4.0, Stroke::new(2.0_f32, crate::style::brand()), StrokeKind::Inside);
                painter.text(
                    r.center(),
                    Align2::CENTER_CENTER,
                    self.settings.language.t("Dateien hier ablegen – Pfad wird eingefügt", "Drop files here – path will be inserted"),
                    FontId::proportional(16.0),
                    Color32::WHITE,
                );
            }
        }
        if files.is_empty() {
            return;
        }
        let id = target.map(|t| t.0).unwrap_or(self.focused);
        if self.entries.get(&id).is_some_and(|e| e.is_chat()) {
            // Chat: Bilder als Platzhalter `[Image#n]` (#357), andere Dateien als Pfad ins Eingabefeld (mit Leerzeichen
            // in Anführungszeichen).
            let paths: Vec<std::path::PathBuf> = files.iter().filter_map(|f| f.path.clone()).collect();
            let (imgs, other): (Vec<_>, Vec<_>) = paths.into_iter().partition(|p| crate::chat::images::is_image_name(&p.to_string_lossy()));
            let text: Vec<String> = other.iter().map(|p| quote_path(&p.to_string_lossy())).collect();
            if let Some(e) = self.entries.get_mut(&id).filter(|_| !text.is_empty()) {
                e.insert_text(&text.join(" "), false);
            }
            self.chat_add_image_files(ctx, id, imgs);
        } else if let Some(s) = self.entries.get(&id).and_then(|e| e.live.as_ref()) {
            // Für die Shell umgerechnet und gequotet (#392); eine Datei einer anderen WSL-Distribution nicht (Hinweis).
            let mut text: Vec<String> = Vec::new();
            let mut refused = None;
            for p in files.iter().filter_map(|f| f.path.as_ref()) {
                match crate::targetpath::for_insert(&s.spec, &p.to_string_lossy()) {
                    Ok(t) => text.push(t),
                    Err(e) => refused = Some(e),
                }
            }
            if !text.is_empty() {
                s.send_text(&format!("{} ", text.join(" ")), false);
            }
            if let Some(e) = refused {
                self.path_refused_toast(&e);
            }
        }
        self.activate(id);
    }

    /// Abgedockte Sitzungen als eigene Fenster.
    pub(super) fn detached_windows(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let theme = self.theme();
        let font = FontId::monospace(self.settings.font_size);
        let lang = self.settings.language;
        for id in self.detached.clone() {
            let Some(name) = self.entries.get(&id).map(|e| e.display_name()) else { continue };
            let vid = egui::ViewportId::from_hash_of(("detached", id));
            let builder = egui::ViewportBuilder::default()
                .with_title(format!("{name} – Paneldeck"))
                .with_inner_size([900.0, 600.0])
                .with_min_inner_size([320.0, 200.0])
                .with_transparent(true);
            let mut close = false;
            ctx.show_viewport_immediate(vid, builder, |ctx, _class| {
                if ctx.input(|i| i.viewport().close_requested()) {
                    close = true;
                }
                // Rückfrage „Neu laden?“ aus diesem Fenster hier zeigen – vor dem Terminal, damit Enter/Esc ihr gehören.
                self.reload_ask_ui(ctx, actions, Some(id));
                // Ebenso die Rückfrage vor dem Einfügen mehrerer Zeilen.
                self.paste_ask_ui(ctx, Some(id));
                // Fokusmeldung (DECSET 1004): die abgedockte Sitzung hat den Fokus, wenn ihr Fenster ihn hat.
                if let Some(s) = self.entries.get(&id).and_then(|e| e.live.as_ref()) {
                    s.report_focus(ctx.input(|i| i.viewport().focused.unwrap_or(false)));
                }
                // Ebenso die Rückfrage „Link öffnen?“ (OSC 8), wenn der Link in diesem Fenster angeklickt wurde.
                self.link_ask_ui(ctx, id);
                if ctx.input_mut(|i| i.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::W)) {
                    close = true;
                }
                egui::CentralPanel::default().frame(egui::Frame::NONE.fill(theme.bg)).show(ctx, |ui| {
                    let rect = ui.available_rect_before_wrap();
                    // Eigene Koordinaten des abgedockten Fensters: nicht in die Trefferflächen des Hauptfensters
                    // (sonst landeten Fokuswechsel, Datei-Drops und Ziehen dort und holten die Sitzung doppelt ins Layout).
                    let mut own_rects = HashMap::new();
                    let mut cx = PaneCx {
                        entries: &mut self.entries,
                        theme: &theme,
                        font: &font,
                        focused: id,
                        want_focus: &mut self.want_focus,
                        actions,
                        lang,
                        claude_mode: self.settings.claude_mode,
                        rects: &mut own_rects,
                        broadcast: false,
                        usage: &self.usage,
                        git: &self.git,
                        dragging: &mut self.dragging,
                        detached: true,
                        themes: &self.themes,
                        profiles: &self.settings.profiles,
                        tl: &self.tlinks,
                        alt_wheel: self.settings.alt_screen_wheel,
                        hide_key: "",
                    };
                    show_pane(ui, id, rect, &mut cx);
                });
            });
            if close {
                actions.push(Action::Attach(id));
            }
        }
    }

    pub(super) fn to_tray(&mut self, ctx: &egui::Context) {
        match self.hwnd {
            Some(h) => {
                crate::platform::window::set_tray_hidden(h, true);
                self.in_tray = true;
            }
            None => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
        }
    }

    pub(super) fn from_tray(&mut self, ctx: &egui::Context) {
        if let (Some(h), true) = (self.hwnd, self.in_tray) {
            crate::platform::window::set_tray_hidden(h, false);
            crate::platform::chrome::apply_corners(h); // nach dem Wiederherstellen aus dem Tray die runden Ecken erneut verlangen
        }
        self.in_tray = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    pub(super) fn handle_tray(&mut self, ctx: &egui::Context) {
        while let Ok(cmd) = self.tray_rx.try_recv() {
            match cmd {
                TrayCmd::Show => self.from_tray(ctx),
                TrayCmd::Quit => {
                    self.quitting = true;
                    if self.in_tray {
                        self.from_tray(ctx);
                    }
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        if self.tray.is_some() && !self.quitting {
            if self.settings.close_to_tray && ctx.input(|i| i.viewport().close_requested()) {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.to_tray(ctx);
            } else if self.settings.minimize_to_tray && !self.in_tray && ctx.input(|i| i.viewport().minimized == Some(true)) {
                self.to_tray(ctx);
            }
        }
        if let Some(t) = &mut self.tray {
            let waiting = self.entries.values().filter(|e| e.needs_you()).count();
            let lang = self.settings.language;
            let tip = if waiting > 0 {
                format!("Paneldeck – {} {}", waiting, lang.t("warten auf dich", "waiting for you"))
            } else {
                "Paneldeck".to_string()
            };
            t.set_tooltip(&tip);
        }
    }
}

/// Ablage-Zone eines Bereichs unter dem Zeiger: 0 Mitte (ersetzen/tauschen), 1 links, 2 rechts, 3 oben, 4 unten.
pub(super) fn drop_zone(r: Rect, p: Pos2) -> u8 {
    let (fx, fy) = ((p.x - r.min.x) / r.width(), (p.y - r.min.y) / r.height());
    if fx < 0.25 {
        1
    } else if fx > 0.75 {
        2
    } else if fy < 0.25 {
        3
    } else if fy > 0.75 {
        4
    } else {
        0
    }
}

/// Pfad für ein Chat-Eingabefeld: mit Leerzeichen in "…".
fn quote_path(p: &str) -> String {
    if p.contains(char::is_whitespace) { format!("\"{p}\"") } else { p.to_string() }
}
