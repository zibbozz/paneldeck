//! Seitenleiste: Sitzungsliste und Ordnerbaum.

use super::*;

impl App {
    pub(super) fn sidebar(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let lang = self.settings.language;
        let panel_fill = ctx.style().visuals.panel_fill;
        let panel = egui::SidePanel::left("sidebar")
            .resizable(true)
            .default_width(self.settings.sidebar_width)
            .width_range(210.0..=460.0)
            .frame(egui::Frame::new().fill(panel_fill).inner_margin(Margin::symmetric(10, 10)))
            .show(ctx, |ui| {
                // Fußzeile: Einstellungen + Gesamtverbrauch
                egui::TopBottomPanel::bottom("sidebar_footer").frame(egui::Frame::NONE).show_separator_line(false).show_inside(ui, |ui| {
                    ui.add_space(S1);
                    ui.horizontal(|ui| {
                        if style::icon_btn(ui, SIcon::Gear, lang.t("Einstellungen", "Settings"), self.settings_open).clicked() {
                            self.settings_open = !self.settings_open;
                        }
                        if self.mcp_chip(ui) {
                            self.settings_open = true;
                            self.settings_tab = 7;
                        }
                        let (mut tok, mut cost) = (0u64, 0.0f64);
                        if self.settings.claude_mode {
                            for id in self.entries.keys().copied().collect::<Vec<_>>() {
                                if let Some(u) = self.usage.get(id) {
                                    tok += u.total.input + u.total.output + u.total.cache_read + u.total.cache_write;
                                    cost += u.cost();
                                }
                            }
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let txt = if tok > 0 {
                                format!("Σ {} {} · ≈ ${:.2}", fmt_tokens(tok), lang.t("Token", "tokens"), cost)
                            } else {
                                format!("Paneldeck {}", env!("CARGO_PKG_VERSION"))
                            };
                            if tok > 0 {
                                let r = ui.add(egui::Label::new(RichText::new(txt).small().color(style::dim(ui))).sense(Sense::click()));
                                if r.on_hover_text(lang.t("Kosten-Übersicht öffnen", "Open cost overview")).clicked() {
                                    actions.push(Action::OpenCosts);
                                }
                            } else {
                                ui.label(RichText::new(txt).small().color(style::dim(ui)));
                            }
                        });
                    });
                });

                // Werkzeugleiste
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(2.0, 2.0);
                    if style::icon_btn(ui, SIcon::Plus, &format!("{} ({})", lang.t("Neue Sitzung …", "New session …"), self.binding_text("new")), false).clicked() {
                        actions.push(Action::NewDialog(false));
                    }
                    if self.settings.claude_mode && style::icon_btn(ui, SIcon::Spark, lang.t("Neue Claude-Sitzung …", "New Claude session …"), false).clicked() {
                        actions.push(Action::NewDialog(true));
                    }
                    if style::icon_btn(ui, SIcon::Queue, &format!("{} ({})", lang.t("Prompt-Composer", "Prompt composer"), self.binding_text("composer")), self.composer_open).clicked() {
                        actions.push(Action::ToggleComposer);
                    }
                    if style::icon_btn(ui, SIcon::Search, &format!("{} ({})", lang.t("Befehle", "Commands"), self.binding_text("palette")), false).clicked() {
                        actions.push(Action::OpenPalette);
                    }
                    if style::icon_btn(ui, SIcon::Prompt, lang.t("Prompt-Bibliothek", "Prompt library"), self.prompts_open).clicked() {
                        actions.push(Action::OpenPrompts);
                    }
                    if style::icon_btn(ui, SIcon::Folder, &format!("{} ({})", lang.t("Dateibaum", "File tree"), self.binding_text("files")), self.files_open).clicked() {
                        actions.push(Action::ToggleFiles);
                    }
                    if style::icon_btn(ui, SIcon::Diff, &format!("{} ({})", lang.t("Änderungen (Diff, Stage, Commit)", "Changes (diff, stage, commit)"), self.binding_text("diff")), self.diff_open).clicked() {
                        actions.push(Action::ToggleDiff);
                    }
                    if self.settings.claude_mode && style::icon_btn(ui, SIcon::Doc, &format!("{} ({})", lang.t("CLAUDE.md / Claude-Dateien", "CLAUDE.md / Claude files"), self.binding_text("editor")), self.editor.is_some()).clicked() {
                        actions.push(Action::OpenEditor);
                    }
                    if self.settings.claude_mode && style::icon_btn(ui, SIcon::Clock, lang.t("Claude-Verlauf", "Claude history"), self.history_open).clicked() {
                        actions.push(Action::OpenHistory);
                    }
                    if style::icon_btn(ui, SIcon::Template, lang.t("Sitzungs-Vorlagen", "Session templates"), self.templates_open).clicked() {
                        actions.push(Action::OpenTemplates);
                    }
                    if style::icon_btn(ui, SIcon::Broadcast, &format!("{} ({})", lang.t("Broadcast an alle sichtbaren Bereiche", "Broadcast to all visible panes"), self.binding_text("broadcast")), self.broadcast).clicked() {
                        actions.push(Action::ToggleBroadcast);
                    }
                });
                ui.add_space(S2);
                self.project_bar(ui, actions);
                ui.add_space(S1);
                self.inbox_ui(ui, actions);

                // Filter
                self.sync_folders();
                self.folder_rects.clear();
                self.tree_geo = TreeGeo::default();
                let tree_on = self.projects.iter().find(|p| p.id == self.active_project).is_some_and(|p| p.tree_view);
                egui::Frame::new().fill(ui.visuals().extreme_bg_color).corner_radius(CornerRadius::same(style::RADIUS)).inner_margin(Margin::symmetric(8, 3)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                        style::paint_icon(ui.painter(), SIcon::Search, r.center(), style::dim(ui));
                        ui.add(
                            egui::TextEdit::singleline(&mut self.filter)
                                .frame(false)
                                .hint_text(lang.t("Filtern: Name oder #Tag", "Filter: name or #tag"))
                                .desired_width((ui.available_width() - 66.0).max(40.0)),
                        );
                        ui.spacing_mut().item_spacing.x = 0.0;
                        if tree_on && style::icon_btn(ui, SIcon::FolderPlus, lang.t("Neuer Ordner", "New folder"), false).clicked() {
                            actions.push(Action::FolderNew(String::new()));
                        }
                        let (icon, tip) = if tree_on {
                            (SIcon::List, lang.t("Als Liste anzeigen", "Show as list"))
                        } else {
                            (SIcon::Tree, lang.t("Als Ordnerbaum anzeigen", "Show as folder tree"))
                        };
                        if style::icon_btn(ui, icon, tip, tree_on).clicked() {
                            actions.push(Action::ToggleTree);
                        }
                    });
                });
                ui.add_space(S2);

                let mut groups: Vec<String> = self.entries.values().filter(|e| self.sidebar_match(e)).map(|e| e.meta.group.clone()).collect();
                groups.sort();
                groups.dedup();
                groups.sort_by_key(|g| !g.is_empty()); // ohne Gruppe zuerst
                let empty = groups.is_empty();

                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    if empty && !(tree_on && self.filter.trim().is_empty()) {
                        ui.add_space(S3);
                        ui.label(RichText::new(if self.filter.trim().is_empty() {
                            lang.t("Noch keine Sitzungen in diesem Projekt.", "No sessions in this project yet.")
                        } else {
                            lang.t("Keine Treffer.", "No matches.")
                        }).color(style::dim(ui)));
                    }
                    if tree_on && self.filter.trim().is_empty() {
                        self.folder_rects.push((String::new(), ui.clip_rect()));
                        self.tree_ui(ui, actions);
                        return;
                    }
                    for g in groups {
                        let ids: Vec<u64> = self
                            .entries
                            .iter()
                            .filter(|(_, e)| e.meta.group == g && self.sidebar_match(e))
                            .map(|(i, _)| *i)
                            .collect();
                        if g.is_empty() {
                            for id in ids {
                                self.session_row(ui, id, actions);
                            }
                        } else {
                            let title = RichText::new(format!("{}  {}", g.to_uppercase(), ids.len())).small().color(style::dim(ui));
                            egui::CollapsingHeader::new(title).id_salt(("grp", &g)).default_open(true).show(ui, |ui| {
                                for id in ids {
                                    self.session_row(ui, id, actions);
                                }
                            });
                        }
                    }
                });
            });
        self.settings.sidebar_width = panel.response.rect.width();
        if panel.response.hovered() && ctx.input(|i| i.key_pressed(Key::F2)) {
            actions.push(Action::StartRename(self.focused));
        }
    }

    pub(super) fn tree_ui(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let folders: Vec<state::Folder> = self.projects.iter().find(|p| p.id == self.active_project).map(|p| p.folders.clone()).unwrap_or_default();
        if folders.is_empty() && !self.entries.values().any(|e| e.meta.project == self.active_project) {
            ui.add_space(S3);
            ui.label(RichText::new(self.settings.language.t("Noch keine Sitzungen in diesem Projekt.", "No sessions in this project yet.")).color(style::dim(ui)));
            return;
        }
        let top = ui.cursor().min.y;
        self.tree_geo.right = ui.available_rect_before_wrap().max.x;
        self.tree_level(ui, &folders, "", actions);
        let bottom = ui.cursor().min.y;
        let (x0, x1) = (ui.max_rect().min.x, ui.max_rect().max.x);
        self.tree_geo.full.insert(String::new(), Rect::from_min_max(Pos2::new(x0, top), Pos2::new(x1, bottom)));
        self.tree_geo.head.insert(String::new(), Rect::from_min_max(Pos2::new(x0, top), Pos2::new(x1, top)));
        ui.add_space(40.0);
    }

    pub(super) fn tree_level(&mut self, ui: &mut Ui, folders: &[state::Folder], parent: &str, actions: &mut Vec<Action>) {
        let mut kids: Vec<&state::Folder> = folders.iter().filter(|f| state::folder_parent(&f.path) == parent).collect();
        kids.sort_by_key(|f| state::folder_leaf(&f.path).to_lowercase());
        self.tree_geo.x.insert(parent.to_string(), ui.cursor().min.x);
        for f in kids {
            let top = ui.cursor().min.y;
            self.folder_row(ui, f, actions);
            if !f.collapsed {
                ui.scope(|ui| {
                    ui.style_mut().visuals.indent_has_left_vline = false;
                    ui.style_mut().spacing.indent = 12.0;
                    ui.indent(("fold", &f.path), |ui| {
                        self.tree_level(ui, folders, &f.path, actions);
                    });
                });
            }
            let bottom = ui.cursor().min.y;
            let r = ui.max_rect();
            let full = Rect::from_min_max(Pos2::new(r.min.x, top), Pos2::new(r.max.x, bottom));
            self.folder_rects.push((f.path.clone(), full));
            self.tree_geo.full.insert(f.path.clone(), full);
        }
        let mut ids: Vec<u64> = self.entries.iter().filter(|(_, e)| e.meta.project == self.active_project && e.meta.group == parent).map(|(i, _)| *i).collect();
        ids.sort();
        for id in ids {
            self.session_row(ui, id, actions);
        }
    }

    pub(super) fn folder_row(&mut self, ui: &mut Ui, f: &state::Folder, actions: &mut Vec<Action>) {
        let lang = self.settings.language;
        let ids = self.folder_sessions(&f.path);
        let all_folders: Vec<state::Folder> = self.projects.iter().find(|p| p.id == self.active_project).map(|p| p.folders.clone()).unwrap_or_default();
        let t = ui.input(|i| i.time);
        // Sammelstatus: wartet etwas auf dich > arbeitet etwas
        let mut agg: Option<Color32> = None;
        for id in &ids {
            if let Some(e) = self.entries.get(id) {
                if e.needs_you() {
                    agg = Some(e.status_color(t));
                    break;
                }
                if e.status == ClaudeStatus::Working && !e.exited() {
                    agg = Some(e.status_color(t));
                }
            }
        }
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::click_and_drag());
        if resp.drag_started() {
            self.dragging_folder = Some(f.path.clone());
        }
        self.folder_rects.push((f.path.clone(), rect));
        self.tree_geo.head.insert(f.path.clone(), rect);
        let (text_c, strong_c, weak_c, hover_bg) = {
            let v = ui.visuals();
            (v.text_color(), v.strong_text_color(), v.weak_text_color(), v.widgets.hovered.bg_fill)
        };
        let p = ui.painter().clone();
        let hv = style::anim(ui.ctx(), resp.id.with("h"), resp.hovered(), style::D_FAST);
        p.rect_filled(rect, 6.0, hover_bg.gamma_multiply(hv));
        let cy = rect.center().y;
        let cx = rect.min.x + 10.0;
        let pts = if f.collapsed {
            vec![Pos2::new(cx - 2.5, cy - 4.0), Pos2::new(cx - 2.5, cy + 4.0), Pos2::new(cx + 3.0, cy)]
        } else {
            vec![Pos2::new(cx - 4.0, cy - 2.5), Pos2::new(cx + 4.0, cy - 2.5), Pos2::new(cx, cy + 3.0)]
        };
        p.add(egui::Shape::convex_polygon(pts, weak_c, Stroke::NONE));
        let fcol = f.color.map(|c| Color32::from_rgb(c[0], c[1], c[2])).unwrap_or(weak_c);
        style::paint_icon(&p, SIcon::Folder, Pos2::new(rect.min.x + 28.0, cy), fcol);
        let mut right = rect.max.x - 8.0;
        let count = p.layout_no_wrap(ids.len().to_string(), FontId::proportional(11.5), weak_c);
        right -= count.size().x;
        p.galley(Pos2::new(right, cy - count.size().y / 2.0), count, weak_c);
        if let Some(c) = agg {
            right -= 12.0;
            p.circle_filled(Pos2::new(right + 3.0, cy), 3.6, c);
        }
        let mut job = egui::text::LayoutJob::simple_singleline(state::folder_leaf(&f.path).to_string(), FontId::proportional(13.0), if resp.hovered() { strong_c } else { text_c });
        job.wrap = egui::text::TextWrapping { max_width: (right - rect.min.x - 48.0).max(20.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
        let g = p.layout_job(job);
        p.galley(Pos2::new(rect.min.x + 42.0, cy - g.size().y / 2.0), g, text_c);
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.clicked() {
            actions.push(Action::FolderToggle(f.path.clone()));
        }
        let path = f.path.clone();
        let resp = resp.on_hover_text(format!("{}\n{}", f.path, if f.cwd.is_empty() { String::new() } else { format!("{}: {}", lang.t("Arbeitsordner", "Working folder"), f.cwd) }).trim().to_string());
        resp.context_menu(|ui| {
            if ui.button(lang.t("Neue Sitzung hier …", "New session here …")).clicked() {
                actions.push(Action::FolderNewSession(path.clone(), false));
                ui.close();
            }
            if self.settings.claude_mode && ui.button(lang.t("Neue Claude-Sitzung hier …", "New Claude session here …")).clicked() {
                actions.push(Action::FolderNewSession(path.clone(), true));
                ui.close();
            }
            if ui.button(lang.t("Neuer Unterordner …", "New subfolder …")).clicked() {
                actions.push(Action::FolderNew(path.clone()));
                ui.close();
            }
            if ui.button(lang.t("Umbenennen …", "Rename …")).clicked() {
                actions.push(Action::FolderRename(path.clone()));
                ui.close();
            }
            ui.menu_button(lang.t("Verschieben nach", "Move to"), |ui| {
                if !state::folder_parent(&path).is_empty() && ui.button(lang.t("Oberste Ebene", "Top level")).clicked() {
                    actions.push(Action::MoveFolder(path.clone(), String::new()));
                    ui.close();
                }
                let mut targets: Vec<&state::Folder> = all_folders.iter().filter(|g| !state::folder_within(&g.path, &path) && g.path != state::folder_parent(&path)).collect();
                targets.sort_by_key(|g| g.path.to_lowercase());
                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    for g in targets {
                        let depth = g.path.matches('/').count();
                        if ui.button(format!("{}{}", "    ".repeat(depth), state::folder_leaf(&g.path))).on_hover_text(&g.path).clicked() {
                            actions.push(Action::MoveFolder(path.clone(), g.path.clone()));
                            ui.close();
                        }
                    }
                });
            });
            if ui.button(lang.t("Als Vorlage speichern …", "Save as template …")).clicked() {
                actions.push(Action::FolderTemplate(path.clone()));
                ui.close();
            }
            ui.separator();
            ui.menu_button(lang.t("Farbe", "Color"), |ui| {
                if ui.button(lang.t("Keine", "None")).clicked() {
                    actions.push(Action::FolderColor(path.clone(), None));
                    ui.close();
                }
                ui.horizontal(|ui| {
                    for c in PALETTE {
                        let (r, resp) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
                        ui.painter().rect_filled(r, 4.0, Color32::from_rgb(c[0], c[1], c[2]));
                        if resp.clicked() {
                            actions.push(Action::FolderColor(path.clone(), Some(c)));
                            ui.close();
                        }
                    }
                });
            });
            ui.menu_button(lang.t("Vorgaben für neue Sitzungen", "Defaults for new sessions"), |ui| {
                let cwd_label = if f.cwd.is_empty() { lang.t("Arbeitsordner wählen …", "Choose working folder …").to_string() } else { format!("{}: {}", lang.t("Arbeitsordner", "Working folder"), f.cwd) };
                if ui.button(cwd_label).clicked() {
                    actions.push(Action::FolderCwd(path.clone(), false));
                    ui.close();
                }
                if !f.cwd.is_empty() && ui.button(lang.t("Arbeitsordner entfernen", "Remove working folder")).clicked() {
                    actions.push(Action::FolderCwd(path.clone(), true));
                    ui.close();
                }
                ui.menu_button(lang.t("Profil", "Profile"), |ui| {
                    if ui.selectable_label(f.profile == 0, lang.t("Erben / keins", "Inherit / none")).clicked() {
                        actions.push(Action::FolderProfile(path.clone(), 0));
                        ui.close();
                    }
                    for pr in &self.settings.profiles {
                        if ui.selectable_label(f.profile == pr.id, &pr.name).clicked() {
                            actions.push(Action::FolderProfile(path.clone(), pr.id));
                            ui.close();
                        }
                    }
                });
            });
            ui.separator();
            if !ids.is_empty() {
                if ui.button(lang.t("Als Raster öffnen", "Open as grid")).clicked() {
                    actions.push(Action::FolderLayout(path.clone()));
                    ui.close();
                }
                if ui.button(lang.t("Beendete Sitzungen neu starten", "Restart ended sessions")).clicked() {
                    actions.push(Action::FolderRestartAll(path.clone()));
                    ui.close();
                }
                if ui.button(lang.t("Prompt an alle senden …", "Send prompt to all …")).clicked() {
                    actions.push(Action::FolderBroadcast(path.clone()));
                    ui.close();
                }
                if armed_button(ui, ("closeall", &path), lang.t("Alle Sitzungen schließen …", "Close all sessions …"), lang.t("Wirklich alle schließen? Nochmal klicken", "Really close all? Click again")) {
                    actions.push(Action::FolderCloseAll(path.clone()));
                    ui.close();
                }
            }
            if armed_button(ui, ("delfolder", &path), lang.t("Ordner löschen (Inhalt bleibt)", "Delete folder (content stays)"), lang.t("Wirklich löschen? Nochmal klicken", "Really delete? Click again")) {
                actions.push(Action::FolderDelete(path.clone()));
                ui.close();
            }
        });
    }

    pub(super) fn session_row(&mut self, ui: &mut Ui, id: u64, actions: &mut Vec<Action>) {
        let lang = self.settings.language;
        let visible = self.layout.as_ref().is_some_and(|l| l.contains(id)) || self.detached.contains(&id);
        let Some(e) = self.entries.get(&id) else { return };
        let name = e.display_name();
        let color = e.color();
        let exited = e.exited();
        let claude = e.meta.claude && self.settings.claude_mode;
        let focused = self.focused == id;
        let attention = e.attention || e.needs_you();

        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 46.0), Sense::click_and_drag());
        self.tree_geo.sess.insert(id, rect);
        if resp.drag_started() {
            self.dragging = Some(id);
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let (text_c, strong_c, weak_c, hover_bg) = {
            let v = ui.visuals();
            (v.text_color(), v.strong_text_color(), v.weak_text_color(), v.widgets.hovered.bg_fill)
        };
        let p = ui.painter().clone();
        let hv = style::anim(ui.ctx(), resp.id.with("h"), resp.hovered(), style::D_FAST);
        let fo = style::anim(ui.ctx(), resp.id.with("f"), focused, style::D_BASE);
        p.rect_filled(rect, 8.0, hover_bg.gamma_multiply(hv * (1.0 - fo)));
        p.rect_filled(rect, 8.0, style::ACCENT.gamma_multiply(0.16 * fo));
        // Farbmarke der Sitzung
        p.rect_filled(Rect::from_min_size(Pos2::new(rect.min.x + 3.0, rect.min.y + 9.0), Vec2::new(3.0, rect.height() - 18.0)), 1.5, color);

        // Statuspunkt (mit Ring, wenn etwas auf dich wartet)
        let t = ui.input(|i| i.time);
        let dot_col = e.status_color(t);
        let dot_pos = Pos2::new(rect.min.x + 20.0, rect.min.y + 17.0);
        p.circle_filled(dot_pos, 4.0, dot_col);
        let ring = style::anim(ui.ctx(), resp.id.with("ring"), attention, style::D_BASE);
        if ring > 0.0 {
            p.circle_stroke(dot_pos, 4.0 + 3.0 * ring, Stroke::new(1.5_f32, dot_col.gamma_multiply(ring)));
        }
        let text_x = rect.min.x + 34.0;
        let right_x = rect.max.x - 10.0;
        let name_y = rect.min.y + 17.0;
        let sub_y = rect.min.y + 33.0;

        // Zeile 2: Status · Ordner/Git · Kosten
        let status_label = e.status_label(lang);
        let mut segs: Vec<(String, Color32)> = Vec::new();
        if !status_label.is_empty() {
            segs.push((status_label.to_string(), dot_col));
        }
        if let Some(g) = self.git.get(id) {
            segs.push((git_line(&g).replacen("git: ", "", 1), weak_c));
        } else {
            let folder = std::path::Path::new(&e.cwd_now()).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
            if !folder.is_empty() {
                segs.push((folder, weak_c));
            }
        }
        if claude {
            if let Some(u) = self.usage.get(id) {
                segs.push((format!("${:.2}", u.cost()), weak_c));
            }
            if !e.meta.queue.is_empty() {
                segs.push((format!("Queue {}{}", e.meta.queue.len(), if e.meta.queue_paused { " ⏸" } else { "" }), style::ACCENT));
            }
        }
        if status_label.is_empty() {
            if let Some(code) = e.live.as_ref().and_then(|s| s.shell_state.lock().unwrap_or_else(|x| x.into_inner()).last_exit) {
                if code != 0 {
                    segs.push((format!("Exit {code}"), Color32::from_rgb(0xe0, 0x6c, 0x75)));
                }
            }
        }
        let has_notes = !e.meta.notes.trim().is_empty();
        let mut status_tip = String::new();
        for part in [e.meta.notes.trim().to_string(), e.status_msg.clone()] {
            if !part.is_empty() {
                status_tip += &part;
                status_tip.push('\n');
            }
        }
        if self.settings.claude_mode {
            if let Some(u) = self.usage.get(id) {
                status_tip += &format!(
                    "{}\n{} {} · ctx {}\n{}: {} in (+{} Cache) · {} out",
                    usage_line(&u, lang),
                    lang.t("Modell", "Model"),
                    u.model,
                    fmt_tokens(u.context),
                    lang.t("Token", "Tokens"),
                    fmt_tokens(u.total.input),
                    fmt_tokens(u.total.cache_read + u.total.cache_write),
                    fmt_tokens(u.total.output),
                );
                status_tip.push('\n');
            }
        }
        if let Some(g) = self.git.get(id) {
            status_tip += &git_line(&g);
        }
        if !e.meta.tags.is_empty() {
            status_tip += &format!("\n#{}", e.meta.tags.join(" #"));
        }
        let status_tip = status_tip.trim().to_string();

        // Rechts oben: Markierungen
        let mut mark_x = right_x;
        if has_notes {
            style::paint_icon(&p, SIcon::Note, Pos2::new(mark_x - 5.0, name_y), weak_c.gamma_multiply(0.9));
            mark_x -= 18.0;
        }
        if visible && !focused {
            p.rect_stroke(Rect::from_center_size(Pos2::new(mark_x - 4.0, name_y), Vec2::new(9.0, 7.0)), 1.5, Stroke::new(1.2_f32, weak_c), StrokeKind::Inside);
            mark_x -= 18.0;
        }
        let text_clip = Rect::from_min_max(Pos2::new(text_x, rect.min.y), Pos2::new(mark_x.max(text_x + 20.0), rect.max.y));

        if let Some((_, buf)) = self.rename.as_mut().filter(|(rid, _)| *rid == id) {
            let edit_rect = Rect::from_min_max(Pos2::new(text_x - 4.0, rect.min.y + 5.0), Pos2::new(right_x, rect.min.y + 29.0));
            let te = ui.put(edit_rect, egui::TextEdit::singleline(buf));
            if !te.has_focus() && !te.lost_focus() {
                te.request_focus();
            }
            let enter = ui.input(|i| i.key_pressed(Key::Enter));
            let esc = ui.input(|i| i.key_pressed(Key::Escape));
            if esc {
                self.rename = None;
                self.want_focus = Some(self.focused);
            } else if enter || te.lost_focus() {
                let (rid, text) = self.rename.take().unwrap_or_default();
                if let Some(en) = self.entries.get_mut(&rid) {
                    let t = text.trim();
                    if t.is_empty() {
                        en.meta.custom_name = false; // zurück zum automatischen Namen
                    } else {
                        en.meta.name = t.to_string();
                        en.meta.custom_name = true;
                    }
                }
                self.want_focus = Some(self.focused);
            }
        } else {
            let name_col = if exited { weak_c } else if focused || attention { strong_c } else { text_c };
            let g = p.layout_no_wrap(name, FontId::proportional(13.5), name_col);
            let mut nx = text_x;
            if claude {
                style::paint_icon(&p, SIcon::SparkSmall, Pos2::new(nx + 4.0, name_y), style::ACCENT);
                nx += 14.0;
            }
            p.with_clip_rect(text_clip).galley(Pos2::new(nx, name_y - g.size().y / 2.0), g, name_col);
            let mut sx = text_x;
            for (i, (txt, col)) in segs.iter().enumerate() {
                if i > 0 {
                    let sep = p.layout_no_wrap("·".to_string(), FontId::proportional(11.5), weak_c);
                    p.with_clip_rect(text_clip).galley(Pos2::new(sx, sub_y - sep.size().y / 2.0), sep.clone(), weak_c);
                    sx += sep.size().x + 5.0;
                }
                let g = p.layout_no_wrap(txt.clone(), FontId::proportional(11.5), *col);
                let w = g.size().x;
                p.with_clip_rect(text_clip).galley(Pos2::new(sx, sub_y - g.size().y / 2.0), g, *col);
                sx += w + 5.0;
            }
            if resp.double_clicked() {
                actions.push(Action::StartRename(id));
            } else if resp.clicked() {
                actions.push(Action::Activate(id));
            }
        }

        let resp = if status_tip.is_empty() { resp } else { resp.on_hover_text(status_tip) };
        let existing_groups: Vec<String> = {
            let mut g: Vec<String> = self.entries.values().map(|e| e.meta.group.clone()).filter(|g| !g.is_empty()).collect();
            if let Some(p) = self.projects.iter().find(|p| p.id == self.active_project) {
                g.extend(p.folders.iter().map(|f| f.path.clone()));
            }
            g.sort_by_key(|x| x.to_lowercase());
            g.dedup();
            g
        };
        resp.context_menu(|ui| {
            if ui.button(lang.t("Umbenennen (F2)", "Rename (F2)")).clicked() {
                actions.push(Action::StartRename(id));
                ui.close();
            }
            ui.menu_button(lang.t("Farbe", "Color"), |ui| {
                ui.horizontal(|ui| {
                    for c in PALETTE {
                        let (r, resp) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
                        ui.painter().rect_filled(r, 4.0, Color32::from_rgb(c[0], c[1], c[2]));
                        if resp.clicked() {
                            actions.push(Action::SetColor(id, c));
                            ui.close();
                        }
                    }
                });
            });
            ui.menu_button(lang.t("In Ordner verschieben", "Move to folder"), |ui| {
                if ui.button(lang.t("Kein Ordner", "No folder")).clicked() {
                    actions.push(Action::SetGroup(id, String::new()));
                    ui.close();
                }
                for g in &existing_groups {
                    if ui.button(g).clicked() {
                        actions.push(Action::SetGroup(id, g.clone()));
                        ui.close();
                    }
                }
                if ui.button(lang.t("Neuer Ordner …", "New folder …")).clicked() {
                    self.group_prompt = Some(Prompt { id, text: String::new() });
                    ui.close();
                }
            });
            if ui.button(lang.t("In eigenem Fenster öffnen", "Open in own window")).clicked() {
                actions.push(Action::Detach(id));
                ui.close();
            }
            if ui.button(lang.t("Tags …", "Tags …")).clicked() {
                actions.push(Action::TagsPrompt(id));
                ui.close();
            }
            if self.settings.claude_mode && ui.button(lang.t("Warteschlange …", "Queue …")).clicked() {
                actions.push(Action::OpenQueue(id));
                ui.close();
            }
            if ui.button(lang.t("Notizen …", "Notes …")).clicked() {
                actions.push(Action::OpenNotes(id));
                ui.close();
            }
            ui.separator();
            if ui.button(lang.t("Verlauf als Datei speichern …", "Save history to file …")).clicked() {
                actions.push(Action::ExportHistory(id));
                ui.close();
            }
            if ui.button(lang.t("Neu starten", "Restart")).clicked() {
                actions.push(Action::Restart(id));
                ui.close();
            }
            if ui.button(lang.t("Duplizieren", "Duplicate")).clicked() {
                actions.push(Action::Duplicate(id));
                ui.close();
            }
            ui.separator();
            if ui.button(lang.t("Schließen", "Close")).clicked() {
                actions.push(Action::RequestClose(id));
                ui.close();
            }
        });
    }
}
