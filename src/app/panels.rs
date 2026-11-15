//! Seitenfenster und Übersichten.

use super::*;

impl App {
    pub(super) fn files_ui(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let lang = self.settings.language;
        let root = self.entries.get(&self.focused).map(|e| e.cwd_now()).unwrap_or_else(home_dir);
        if root != self.tree_root || self.tree_stamp.elapsed() > Duration::from_secs(10) {
            self.tree_cache.clear();
            self.tree_root = root.clone();
            self.tree_stamp = Instant::now();
        }
        let hidden = self.files_hidden;
        egui::SidePanel::right(style::salted(Id::new("files"))).resizable(true).default_width(270.0).width_range(180.0..=520.0).show_animated(ctx, self.files_open, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.strong(lang.t("Dateien", "Files"));
                if ui.small_button(lang.t("Aktualisieren", "Refresh")).clicked() {
                    self.tree_cache.clear();
                }
                if ui.checkbox(&mut self.files_hidden, lang.t("versteckte", "hidden")).changed() {
                    self.tree_cache.clear();
                }
            });
            ui.add(egui::Label::new(RichText::new(&root).weak().small()).truncate());
            ui.weak(lang.t("Doppelklick fügt den Pfad ins Terminal ein · Rechtsklick für mehr", "Double-click inserts the path · right-click for more"));
            ui.separator();
            egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                tree_children(ui, Path::new(&root), hidden, &mut self.tree_cache, actions, lang);
            });
        });
    }

    pub(super) fn queue_ui(&mut self, ctx: &egui::Context) {
        let lang = self.settings.language;
        let Some(id) = self.queue_open else { return };
        if !self.entries.contains_key(&id) {
            self.queue_open = None;
            return;
        }
        let mut open = true;
        let title = format!("{} – {}", lang.t("Warteschlange", "Queue"), self.entries[&id].display_name());
        let mut add: Option<String> = None;
        egui::Window::new(title).id(Id::new("queue_win")).open(&mut open).default_size([520.0, 420.0]).show(ctx, |ui| {
            let Some(e) = self.entries.get_mut(&id) else { return };
            ui.label(RichText::new(lang.t(
                "Die Prompts werden nacheinander gesendet: der nächste, sobald Claude „bereit“ meldet. Bei Freigabe-Fragen wartet die Warteschlange.",
                "Prompts are sent one after another: the next as soon as Claude reports “ready”. It waits during approval requests.",
            )).small().color(style::dim(ui)));
            ui.add_space(S2);
            ui.horizontal(|ui| {
                let label = if e.meta.queue_paused { lang.t("Fortsetzen", "Resume") } else { lang.t("Pausieren", "Pause") };
                if ui.button(label).clicked() {
                    e.meta.queue_paused = !e.meta.queue_paused;
                    if !e.meta.queue_paused && e.status == ClaudeStatus::Idle {
                        e.send_next_queued();
                    }
                }
                if ui.add_enabled(!e.meta.queue.is_empty() && e.status == ClaudeStatus::Idle, egui::Button::new(lang.t("Nächsten jetzt senden", "Send next now"))).clicked() {
                    e.send_next_queued();
                }
                if ui.add_enabled(!e.meta.queue.is_empty(), egui::Button::new(lang.t("Alle löschen", "Clear all"))).clicked() {
                    e.meta.queue.clear();
                }
                if !e.meta.claude {
                    ui.colored_label(Color32::from_rgb(0xe5, 0xc0, 0x7b), lang.t("Keine Claude-Sitzung – die Warteschlange läuft nur bei Claude.", "Not a Claude session – the queue only runs with Claude."));
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().max_height(230.0).auto_shrink([false, true]).show(ui, |ui| {
                let n = e.meta.queue.len();
                let mut remove: Option<usize> = None;
                let mut swap: Option<(usize, usize)> = None;
                for i in 0..n {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{}", i + 1)).color(style::dim(ui)));
                        ui.add(egui::TextEdit::multiline(&mut e.meta.queue[i]).desired_rows(1).desired_width(330.0));
                        if i > 0 && style::icon_btn_small(ui, SIcon::Up, lang.t("Nach oben", "Move up")).clicked() {
                            swap = Some((i, i - 1));
                        }
                        if i + 1 < n && style::icon_btn_small(ui, SIcon::Down, lang.t("Nach unten", "Move down")).clicked() {
                            swap = Some((i, i + 1));
                        }
                        if style::icon_btn_small(ui, SIcon::Close, lang.t("Entfernen", "Remove")).clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if n == 0 {
                    ui.label(RichText::new(lang.t("Die Warteschlange ist leer.", "The queue is empty.")).color(style::dim(ui)));
                }
                if let Some((a, b)) = swap {
                    e.meta.queue.swap(a, b);
                }
                if let Some(i) = remove {
                    e.meta.queue.remove(i);
                }
            });
            ui.separator();
            ui.label(lang.t("Neuer Prompt", "New prompt"));
            ui.add(egui::TextEdit::multiline(&mut self.queue_draft).desired_rows(3).desired_width(f32::INFINITY).hint_text(lang.t("Prompt eingeben …", "Enter prompt …")));
            ui.horizontal(|ui| {
                if ui.add_enabled(!self.queue_draft.trim().is_empty(), egui::Button::new(lang.t("Zur Warteschlange hinzufügen", "Add to queue"))).clicked() {
                    add = Some(std::mem::take(&mut self.queue_draft));
                }
            });
        });
        if let Some(t) = add {
            self.pending_actions.push(Action::QueueAdd(id, t));
        }
        if !open {
            self.queue_open = None;
        }
    }

    /// Karten für Sitzungen, die auf eine Freigabe warten (Seitenleiste).
    pub(super) fn inbox_ui(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let lang = self.settings.language;
        let waiting: Vec<(u64, String, String, bool)> = self
            .entries
            .iter()
            .filter(|(_, e)| e.needs_you() && e.meta.claude)
            .map(|(id, e)| (*id, e.display_name(), e.status_msg.clone(), e.status == ClaudeStatus::Permission))
            .collect();
        if waiting.is_empty() {
            return;
        }
        egui::Frame::new()
            .fill(style::ACCENT.gamma_multiply(0.12))
            .stroke(Stroke::new(1.0_f32, style::ACCENT.gamma_multiply(0.5)))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::same(8))
            .show(ui, |ui| {
                ui.label(RichText::new(format!("{} ({})", lang.t("Wartet auf dich", "Waiting for you"), waiting.len())).strong().small());
                for (id, name, msg, permission) in waiting {
                    ui.add_space(S1);
                    ui.label(RichText::new(name).strong());
                    if !msg.is_empty() {
                        ui.add(egui::Label::new(RichText::new(msg).small().color(style::dim(ui))).wrap());
                    }
                    ui.horizontal(|ui| {
                        if permission {
                            if ui.add(egui::Button::new(RichText::new(lang.t("Ja", "Yes")).strong()).fill(Color32::from_rgb(0x2f, 0x7d, 0x4a))).on_hover_text(lang.t("Sendet Enter (Standardauswahl „Ja“)", "Sends Enter (default choice “Yes”)")).clicked() {
                                actions.push(Action::Approve(id, true));
                            }
                            if ui.button(lang.t("Nein", "No")).on_hover_text(lang.t("Sendet Esc", "Sends Esc")).clicked() {
                                actions.push(Action::Approve(id, false));
                            }
                        }
                        if ui.button(lang.t("Öffnen", "Open")).clicked() {
                            actions.push(Action::Activate(id));
                        }
                    });
                }
            });
        ui.add_space(S2);
    }

    pub(super) fn diff_ui(&mut self, ctx: &egui::Context) {
        let lang = self.settings.language;
        if !self.diff_open {
            self.diff.set_target(None, None);
            return;
        }
        let dir = self.entries.get(&self.focused).map(|e| e.cwd_now()).unwrap_or_else(home_dir);
        let snap = self.diff.snapshot();
        let sel_change: Option<(Change, bool)> = self
            .diff_sel
            .as_ref()
            .and_then(|(p, st)| snap.changes.iter().find(|c| &c.path == p).map(|c| (c.clone(), *st)));
        self.diff.set_target(Some(PathBuf::from(&dir)), sel_change.clone());
        let esc_close = ctx.input(|i| i.key_pressed(Key::Escape));
        let mut open = true;
        let mut toggle: Option<(Change, bool)> = None; // (Datei, jetzt stagen?)
        let mut do_stage_all = false;
        let mut do_commit = false;
        let mut select: Option<(String, bool)> = None;
        egui::Window::new(lang.t("Änderungen", "Changes")).open(&mut open).default_size([960.0, 540.0]).show(ctx, |ui| {
            if let Some(err) = &snap.error {
                ui.colored_label(Color32::from_rgb(0xe5, 0xc0, 0x7b), err);
                ui.label(RichText::new(&dir).small().color(style::dim(ui)));
                return;
            }
            let staged_n = snap.changes.iter().filter(|c| c.staged()).count();
            ui.horizontal(|ui| {
                ui.label(RichText::new(snap.repo.as_ref().map(|r| r.display().to_string()).unwrap_or_default()).small().color(style::dim(ui)));
                ui.label(RichText::new(format!("git: {}", snap.branch)).strong());
                ui.label(RichText::new(format!("{} {} · {} {}", snap.changes.len(), lang.t("geändert", "changed"), staged_n, lang.t("vorgemerkt", "staged"))).small().color(style::dim(ui)));
            });
            ui.separator();
            ui.scope(|ui| {
            ui.set_max_height(340.0);
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(300.0);
                    egui::ScrollArea::vertical().max_height(320.0).id_salt("difflist").show(ui, |ui| {
                        if snap.changes.is_empty() {
                            ui.label(RichText::new(lang.t("Keine Änderungen.", "No changes.")).color(style::dim(ui)));
                        }
                        for c in &snap.changes {
                            ui.horizontal(|ui| {
                                let mut staged = c.staged() && !c.unstaged();
                                if ui.checkbox(&mut staged, "").on_hover_text(lang.t("Vormerken (Stage) / zurücknehmen", "Stage / unstage")).changed() {
                                    toggle = Some((c.clone(), staged));
                                }
                                let (letter, col) = match (c.x, c.y) {
                                    ('?', _) => ('?', Color32::from_rgb(0x98, 0xc3, 0x79)),
                                    ('A', _) | (_, 'A') => ('A', Color32::from_rgb(0x98, 0xc3, 0x79)),
                                    ('D', _) | (_, 'D') => ('D', Color32::from_rgb(0xe0, 0x6c, 0x75)),
                                    ('R', _) => ('R', Color32::from_rgb(0x61, 0xaf, 0xef)),
                                    _ => ('M', Color32::from_rgb(0xe5, 0xc0, 0x7b)),
                                };
                                ui.label(RichText::new(letter.to_string()).monospace().color(col));
                                let is_sel = self.diff_sel.as_ref().is_some_and(|(p, _)| *p == c.path);
                                let both = c.staged() && c.unstaged();
                                let r = ui.selectable_label(is_sel, RichText::new(&c.path).small());
                                if r.clicked() {
                                    select = Some((c.path.clone(), c.staged() && !c.unstaged()));
                                }
                                if both {
                                    ui.label(RichText::new(lang.t("teils", "part")).small().color(style::dim(ui)));
                                }
                            });
                        }
                    });
                });
                ui.separator();
                ui.vertical(|ui| {
                    if let Some((c, staged_view)) = &sel_change {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&c.path).strong());
                            if c.staged() && c.unstaged() {
                                if ui.selectable_label(!*staged_view, lang.t("Nicht vorgemerkt", "Unstaged")).clicked() {
                                    select = Some((c.path.clone(), false));
                                }
                                if ui.selectable_label(*staged_view, lang.t("Vorgemerkt", "Staged")).clicked() {
                                    select = Some((c.path.clone(), true));
                                }
                            }
                        });
                    }
                    let lines: Vec<&str> = snap.diff.lines().collect();
                    let row_h = 15.0;
                    egui::ScrollArea::both().max_height(320.0).auto_shrink([false, false]).id_salt("diffview").show_rows(ui, row_h, lines.len(), |ui, range| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for l in &lines[range] {
                            let col = if l.starts_with('+') && !l.starts_with("+++") {
                                Color32::from_rgb(0x98, 0xc3, 0x79)
                            } else if l.starts_with('-') && !l.starts_with("---") {
                                Color32::from_rgb(0xe0, 0x6c, 0x75)
                            } else if l.starts_with("@@") {
                                Color32::from_rgb(0x61, 0xaf, 0xef)
                            } else {
                                style::dim(ui)
                            };
                            ui.add(egui::Label::new(RichText::new(*l).monospace().size(12.0).color(col)).extend());
                        }
                    });
                    if sel_change.is_none() {
                        ui.label(RichText::new(lang.t("Datei links auswählen, um den Diff zu sehen.", "Select a file on the left to see its diff.")).color(style::dim(ui)));
                    }
                });
            });
            });
            ui.separator();
            ui.add(egui::TextEdit::multiline(&mut self.commit_msg).desired_rows(2).desired_width(f32::INFINITY).hint_text(lang.t("Commit-Nachricht …", "Commit message …")));
            ui.horizontal(|ui| {
                if ui.add_enabled(!snap.changes.is_empty(), egui::Button::new(lang.t("Alle vormerken", "Stage all"))).clicked() {
                    do_stage_all = true;
                }
                if ui.add_enabled(staged_n > 0 && !self.commit_msg.trim().is_empty(), egui::Button::new(RichText::new(format!("{} ({staged_n})", lang.t("Committen", "Commit"))).strong())).clicked() {
                    do_commit = true;
                }
                if ui.button(lang.t("Aktualisieren", "Refresh")).clicked() {
                    self.diff.refresh();
                }
                ui.label(RichText::new(lang.t("Kein Push – das übernimmst du im Terminal.", "No push – do that in the terminal.")).small().color(style::dim(ui)));
            });
        });
        if let Some(s) = select {
            self.diff_sel = Some(s);
            self.diff.refresh();
        }
        let repo = snap.repo.clone();
        if let (Some(repo), Some((c, stage_it))) = (repo.clone(), toggle) {
            let p = c.path.clone();
            self.git_bg(ctx, "", move || if stage_it { gitops::stage(&repo, &p).map(|_| String::new()) } else { gitops::unstage(&repo, &p).map(|_| String::new()) });
        }
        if let (Some(repo), true) = (repo.clone(), do_stage_all) {
            self.git_bg(ctx, lang.t("Alles vorgemerkt", "All staged"), move || gitops::stage_all(&repo).map(|_| String::new()));
        }
        if let (Some(repo), true) = (repo, do_commit) {
            // Nachricht erst nach erfolgreichem Commit leeren (Hook abgelehnt, nichts vorgemerkt … → bleibt stehen).
            let msg = self.commit_msg.clone();
            let done = self.commit_ok.clone();
            self.git_bg(ctx, lang.t("Commit erstellt", "Commit created"), move || {
                let r = gitops::commit(&repo, msg.trim());
                if r.is_ok() {
                    done.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                r
            });
        }
        if !open || (esc_close && !ctx.wants_keyboard_input()) {
            self.diff_open = false;
        }
    }

    pub(super) fn costs_ui(&mut self, ctx: &egui::Context) {
        if !self.cost_open {
            return;
        }
        let lang = self.settings.language;
        let mut open = true;
        // Daten sammeln
        let mut per_session: Vec<(u64, String, f64, u64)> = Vec::new();
        let mut day_cost: std::collections::BTreeMap<String, f64> = Default::default();
        for (id, e) in &self.entries {
            if let Some(u) = self.usage.get(*id) {
                let tok = u.total.input + u.total.output + u.total.cache_read + u.total.cache_write;
                per_session.push((*id, e.display_name(), u.cost(), tok));
                for (d, c) in u.cost_by_day() {
                    *day_cost.entry(d).or_default() += c;
                }
            }
        }
        let now = usage::today_secs();
        let today = usage::utc_date(now);
        let days: Vec<(String, f64)> = (0..14).rev().map(|i| { let d = usage::utc_date(now.saturating_sub(i * 86400)); let c = day_cost.get(&d).copied().unwrap_or(0.0); (d, c) }).collect();
        let week: f64 = days.iter().rev().take(7).map(|d| d.1).sum::<f64>() + 0.0;
        let total: f64 = per_session.iter().map(|s| s.2).sum::<f64>() + 0.0;
        let today_c = days.last().map(|d| d.1).unwrap_or(0.0);
        let budget = self.settings.daily_budget;
        egui::Window::new(lang.t("Kosten-Übersicht", "Cost overview")).open(&mut open).default_size([560.0, 560.0]).show(ctx, |ui| {
            ui.label(RichText::new(lang.t("Geschätzt aus den lokalen Transkripten der Sitzungen in Paneldeck (nur Sitzungen, die hier geöffnet sind). Tage in UTC.", "Estimated from the local transcripts of sessions in Paneldeck (only sessions open here). Days in UTC.")).small().color(style::dim(ui)));
            ui.add_space(S1);
            ui.horizontal(|ui| {
                for (label, v) in [(lang.t("Heute", "Today"), today_c), (lang.t("7 Tage", "7 days"), week), (lang.t("Gesamt", "Total"), total)] {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(format!("≈ ${v:.2}")).heading());
                        ui.label(RichText::new(label).small().color(style::dim(ui)));
                    });
                    ui.add_space(S3);
                }
            });
            if budget > 0.0 {
                let frac = (today_c / budget).clamp(0.0, 1.0) as f32;
                ui.add(egui::ProgressBar::new(frac).text(format!("{} ${:.2} / ${:.2}", lang.t("Tagesbudget", "Daily budget"), today_c, budget)).fill(if today_c > budget { Color32::from_rgb(0xe0, 0x4f, 0x5f) } else { style::ACCENT }));
            }
            ui.add_space(S2);
            ui.label(RichText::new(lang.t("Letzte 14 Tage", "Last 14 days")).strong());
            let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 110.0), Sense::hover());
            let p = ui.painter();
            let max = days.iter().map(|d| d.1).fold(0.0001_f64, f64::max);
            let bw = rect.width() / days.len() as f32;
            let weak = style::dim(ui);
            for (i, (d, c)) in days.iter().enumerate() {
                let h = ((*c / max) as f32 * (rect.height() - 34.0)).max(if *c > 0.0 { 2.0 } else { 0.0 });
                let x0 = rect.min.x + i as f32 * bw;
                let bar = Rect::from_min_max(Pos2::new(x0 + bw * 0.18, rect.max.y - 16.0 - h), Pos2::new(x0 + bw * 0.82, rect.max.y - 16.0));
                let col = if *d == today { style::ACCENT } else { style::ACCENT.gamma_multiply(0.55) };
                p.rect_filled(bar, 2.0, col);
                if *c > 0.0 {
                    p.text(Pos2::new(bar.center().x, bar.min.y - 2.0), Align2::CENTER_BOTTOM, format!("{c:.2}"), FontId::proportional(9.5), weak);
                }
                p.text(Pos2::new(bar.center().x, rect.max.y - 2.0), Align2::CENTER_BOTTOM, &d[5..], FontId::proportional(9.5), weak);
            }
            ui.add_space(S2);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                // Nach Projekt
                ui.label(RichText::new(lang.t("Nach Projekt", "By project")).strong());
                for pr in &self.projects {
                    let c: f64 = per_session.iter().filter(|s| self.entries.get(&s.0).is_some_and(|e| e.meta.project == pr.id)).map(|s| s.2).sum::<f64>() + 0.0;
                    ui.horizontal(|ui| {
                        ui.label(&pr.name);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(format!("≈ ${c:.2}")));
                    });
                }
                // Nach Ordner (aktives Projekt)
                if let Some(pr) = self.projects.iter().find(|p| p.id == self.active_project) {
                    if !pr.folders.is_empty() {
                        ui.add_space(S2);
                        ui.label(RichText::new(format!("{} {}", lang.t("Nach Ordner in", "By folder in"), lang.q(&pr.name))).strong());
                        let mut fs: Vec<&state::Folder> = pr.folders.iter().collect();
                        fs.sort_by_key(|f| f.path.to_lowercase());
                        for f in fs {
                            let c: f64 = per_session.iter().filter(|s| self.entries.get(&s.0).is_some_and(|e| e.meta.project == pr.id && !e.meta.group.is_empty() && state::folder_within(&e.meta.group, &f.path))).map(|s| s.2).sum::<f64>() + 0.0;
                            ui.horizontal(|ui| {
                                ui.add_space(f.path.matches('/').count() as f32 * 14.0);
                                ui.label(state::folder_leaf(&f.path));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(format!("≈ ${c:.2}")));
                            });
                        }
                    }
                }
                // Nach Sitzung
                ui.add_space(S2);
                ui.label(RichText::new(lang.t("Teuerste Sitzungen", "Most expensive sessions")).strong());
                per_session.sort_by(|a, b| b.2.total_cmp(&a.2));
                for (_, name, c, tok) in per_session.iter().take(15) {
                    ui.horizontal(|ui| {
                        ui.add(egui::Label::new(name).truncate());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(format!("≈ ${c:.2} · {} {}", fmt_tokens(*tok), lang.t("Token", "tokens")));
                        });
                    });
                }
            });
        });
        self.cost_open = open && !ctx.input(|i| i.key_pressed(Key::Escape));
    }

    pub(super) fn history_ui(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        if !self.history_open {
            return;
        }
        let lang = self.settings.language;
        let mut open = true;
        egui::Window::new(lang.t("Claude-Verlauf", "Claude history"))
            .open(&mut open)
            .default_size([680.0, 460.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let te = ui.add(egui::TextEdit::singleline(&mut self.history_filter).hint_text(lang.t("Filtern (Prompt oder Ordner) …", "Filter (prompt or folder) …")).desired_width(300.0));
                    let enter = te.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    let can = self.history_filter.trim().chars().count() >= 2 && self.history_hits_rx.is_none();
                    if ui.add_enabled(can, egui::Button::new(lang.t("Volltext", "Full text"))).on_hover_text(lang.t("Durchsucht alle Konversationen (Enter)", "Searches all conversations (Enter)")).clicked() || (enter && can) {
                        self.history_hits_rx = Some(history::search_async(ctx.clone(), self.history_filter.clone()));
                        self.history_hits = None;
                    }
                    if (self.history_hits.is_some() || self.history_hits_rx.is_some()) && ui.button("✕").on_hover_text(lang.t("Suche beenden", "Clear search")).clicked() {
                        self.history_hits = None;
                        self.history_hits_rx = None;
                    }
                    if ui.button(lang.t("Neu laden", "Reload")).clicked() {
                        self.history_rx = Some(history::scan_async(ctx.clone()));
                    }
                    if self.history_rx.is_some() || self.history_hits_rx.is_some() {
                        ui.spinner();
                    }
                });
                ui.separator();
                if let Some(hits) = &self.history_hits {
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                        ui.weak(format!("{} {}", hits.len(), lang.t("Treffer im Volltext", "full-text matches")));
                        if hits.is_empty() {
                            ui.weak(lang.t("Nichts gefunden.", "Nothing found."));
                        }
                        for (h, sn) in hits {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button(lang.t("Fortsetzen", "Resume")).clicked() {
                                    actions.push(Action::ResumeItem(h.clone()));
                                }
                                ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                                    ui.add(egui::Label::new(RichText::new(&h.title).strong()).truncate());
                                    ui.add(egui::Label::new(RichText::new(sn).small()).truncate());
                                    ui.weak(format!("{} · {}", h.cwd, history::age_label(h.modified, lang == Lang::De)));
                                });
                            });
                            ui.separator();
                        }
                    });
                    return;
                }
                let f = self.history_filter.to_lowercase();
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    if self.history.is_empty() && self.history_rx.is_none() {
                        ui.weak(lang.t("Keine Konversationen gefunden (~/.claude/projects).", "No conversations found (~/.claude/projects)."));
                    }
                    for (i, h) in self.history.iter().enumerate() {
                        if !f.is_empty() && !h.title.to_lowercase().contains(&f) && !h.cwd.to_lowercase().contains(&f) {
                            continue;
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(lang.t("Fortsetzen", "Resume")).clicked() {
                                actions.push(Action::ResumeHistory(i));
                            }
                            ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                                ui.add(egui::Label::new(RichText::new(&h.title).strong()).truncate());
                                ui.weak(format!("{} · {}", h.cwd, history::age_label(h.modified, lang == Lang::De)));
                            });
                        });
                        ui.separator();
                    }
                });
            });
        self.history_open = open;
    }

    pub(super) fn gsearch_ui(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let lang = self.settings.language;
        let Some(mut g) = self.gsearch.take() else { return };
        let now = ctx.input(|i| i.time);
        let mut open = !ctx.input(|i| i.key_pressed(Key::Escape));
        let mut jump: Option<(u64, i32)> = None;
        egui::Window::new(lang.t("Alle Sitzungen durchsuchen", "Search all sessions"))
            .open(&mut open)
            .default_size([640.0, 440.0])
            .show(ctx, |ui| {
                let te = ui.add(egui::TextEdit::singleline(&mut g.query).desired_width(f32::INFINITY).hint_text(lang.t("Suchbegriff (mind. 2 Zeichen) …", "Search term (min. 2 characters) …")));
                if !g.opened {
                    te.request_focus();
                    g.opened = true;
                }
                if te.changed() {
                    g.last_edit = now;
                    ctx.request_repaint_after(Duration::from_millis(350));
                }
                // Entprellt suchen
                if g.query != g.done_for && (now - g.last_edit > 0.3 || ctx.input(|i| i.key_pressed(Key::Enter))) {
                    g.done_for = g.query.clone();
                    g.hits.clear();
                    if g.query.trim().chars().count() >= 2 {
                        for (id, e) in &self.entries {
                            let Some(s) = e.live.as_ref() else { continue };
                            let t = s.term.lock().unwrap_or_else(|e| e.into_inner());
                            for m in search::find_all(&t, g.query.trim()).into_iter().rev().take(40) {
                                g.hits.push(GsHit { session: *id, line: m.line, text: search::line_text(&t, m.line) });
                            }
                        }
                    }
                }
                ui.add_space(S1);
                ui.label(RichText::new(format!("{} {}", g.hits.len(), lang.t("Treffer", "hits"))).small().color(style::dim(ui)));
                ui.separator();
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    let mut last = 0u64;
                    for h in &g.hits {
                        if h.session != last {
                            last = h.session;
                            let name = self.entries.get(&h.session).map(|e| e.display_name()).unwrap_or_default();
                            ui.add_space(S1);
                            ui.label(RichText::new(name).strong());
                        }
                        let r = ui.add(egui::Button::new(RichText::new(h.text.trim()).monospace().small()).frame(false).wrap_mode(egui::TextWrapMode::Truncate));
                        if r.clicked() {
                            jump = Some((h.session, h.line));
                        }
                    }
                });
            });
        if let Some((sid, line)) = jump {
            actions.push(Action::GlobalJump(sid, line, g.query.trim().to_string()));
        }
        if open {
            self.gsearch = Some(g);
        }
    }

    pub(super) fn palette_items(&self) -> Vec<(String, String, Action)> {
        let lang = self.settings.language;
        let f = self.focused;
        let mut v: Vec<(String, String, Action)> = vec![
            (lang.t("Neue Sitzung (Standard-Shell)", "New session (default shell)").into(), "Strg+Umschalt+T".into(), Action::NewQuick),
            (lang.t("Neue Sitzung …", "New session …").into(), String::new(), Action::NewDialog(false)),
        ];
        if self.settings.claude_mode {
            v.push((lang.t("Neue Claude-Sitzung …", "New Claude session …").into(), String::new(), Action::NewDialog(true)));
        }
        v.push((lang.t("Vorlagen …", "Templates …").into(), String::new(), Action::OpenTemplates));
        for (i, t) in self.templates.iter().enumerate() {
            v.push((format!("{} {}", lang.t("Vorlage starten:", "Start template:"), t.name), String::new(), Action::TemplateStart(i)));
        }
        if self.settings.claude_mode {
            v.push((lang.t("Kosten-Übersicht", "Cost overview").into(), String::new(), Action::OpenCosts));
        }
        v.extend([
            (lang.t("Bereich rechts teilen", "Split pane right").into(), "Strg+Umschalt+D".into(), Action::Split(f, Dir::Horizontal)),
            (lang.t("Bereich darunter teilen", "Split pane below").into(), "Strg+Umschalt+E".into(), Action::Split(f, Dir::Vertical)),
            (lang.t("Bereich ausblenden", "Hide pane").into(), "Strg+Umschalt+W".into(), Action::HidePane(f)),
            (lang.t("Sitzung umbenennen", "Rename session").into(), "Strg+Umschalt+R".into(), Action::StartRename(f)),
            (lang.t("Sitzung neu starten", "Restart session").into(), String::new(), Action::Restart(f)),
            (lang.t("Sitzung schließen", "Close session").into(), String::new(), Action::RequestClose(f)),
            (lang.t("Alle Sitzungen durchsuchen", "Search all sessions").into(), self.binding_text("search_all"), Action::OpenGlobalSearch),
            (lang.t("Verlauf als Datei speichern", "Save history to file").into(), String::new(), Action::ExportHistory(f)),
            (lang.t("Zum vorherigen Befehl springen", "Jump to previous command").into(), self.binding_text("prev_cmd"), Action::JumpCmd(-1)),
            (lang.t("Zum nächsten Befehl springen", "Jump to next command").into(), self.binding_text("next_cmd"), Action::JumpCmd(1)),
            (lang.t("Im Verlauf suchen", "Search scrollback").into(), "Strg+Umschalt+F".into(), Action::OpenSearch),
            (lang.t("Broadcast an/aus", "Toggle broadcast").into(), "Strg+Umschalt+B".into(), Action::ToggleBroadcast),
            (lang.t("Prompt-Composer", "Prompt composer").into(), self.binding_text("composer"), Action::ToggleComposer),
            (lang.t("Warteschlange der Sitzung", "Session queue").into(), self.binding_text("queue"), Action::OpenQueue(f)),
            (lang.t("Änderungen (Diff) ein/aus", "Toggle changes (diff)").into(), self.binding_text("diff"), Action::ToggleDiff),
            (lang.t("Notizen der Sitzung", "Session notes").into(), "Strg+Umschalt+N".into(), Action::OpenNotes(f)),
            (lang.t("Prompt-Bibliothek", "Prompt library").into(), String::new(), Action::OpenPrompts),
            (lang.t("Sitzung in eigenem Fenster öffnen", "Open session in own window").into(), String::new(), Action::Detach(f)),
            (lang.t("CLAUDE.md / Claude-Dateien bearbeiten", "Edit CLAUDE.md / Claude files").into(), self.binding_text("editor"), Action::OpenEditor),
            (lang.t("Dateibaum ein/aus", "Toggle file tree").into(), "Strg+Umschalt+O".into(), Action::ToggleFiles),
            (lang.t("Claude-Verlauf (Konversationen fortsetzen)", "Claude history (resume conversations)").into(), String::new(), Action::OpenHistory),
            (lang.t("Einstellungen", "Settings").into(), String::new(), Action::OpenSettings),
        ]);
        v.push((lang.t("Neues Projekt …", "New project …").into(), String::new(), Action::NewProjectPrompt));
        for (i, p) in self.projects.iter().enumerate() {
            if p.id != self.active_project {
                v.push((format!("{} {}", lang.t("Projekt wechseln:", "Switch project:"), p.name), format!("Strg+Umschalt+{}", i + 1), Action::SwitchProject(p.id)));
            }
        }
        for (id, e) in &self.entries {
            v.push((format!("{} {}", lang.t("Wechseln zu:", "Switch to:"), e.display_name()), e.meta.group.clone(), Action::Activate(*id)));
        }
        for (i, s) in self.prompts.iter().enumerate() {
            v.push((format!("{} {}", lang.t("Prompt senden:", "Send prompt:"), s.name), String::new(), Action::SendSnippet(i)));
        }
        v
    }

    pub(super) fn palette_ui(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(mut p) = self.palette.take() else { return };
        let lang = self.settings.language;
        let (down, up, enter, esc) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        let mut items: Vec<(i32, String, String, Action)> = self
            .palette_items()
            .into_iter()
            .filter_map(|(l, h, a)| if p.query.trim().is_empty() { Some((0, l, h, a)) } else { fuzzy(&p.query, &l).map(|s| (s, l, h, a)) })
            .collect();
        if !p.query.trim().is_empty() {
            items.sort_by(|a, b| b.0.cmp(&a.0));
        }
        items.truncate(14);
        // Ticket-Treffer (Experten-Projekte): bei einer Ticketnummer zuerst, sonst nach den Befehlen
        let tickets = self.tlinks_palette_hits(&p.query);
        if !tickets.is_empty() {
            let rows = tickets.into_iter().map(|(l, h, a)| (0, l, h, a));
            if terminal_links::normalize_key_query(&p.query).is_some() {
                items.splice(0..0, rows);
            } else {
                items.extend(rows);
            }
        }
        if down && !items.is_empty() {
            p.sel = (p.sel + 1) % items.len();
        }
        if up && !items.is_empty() {
            p.sel = (p.sel + items.len() - 1) % items.len();
        }
        p.sel = p.sel.min(items.len().saturating_sub(1));
        let mut chosen: Option<Action> = None;
        let mut close = esc;
        if enter {
            chosen = items.get(p.sel).map(|i| i.3.clone());
        }
        egui::Window::new("palette")
            .title_bar(false)
            .resizable(false)
            .anchor(Align2::CENTER_TOP, Vec2::new(0.0, 70.0))
            .fixed_size([560.0, 0.0])
            .show(ctx, |ui| {
                let before = p.query.clone();
                let te = ui.add(
                    egui::TextEdit::singleline(&mut p.query)
                        .desired_width(f32::INFINITY)
                        .hint_text(if self.tlinks.enabled {
                            lang.t("Befehl, Sitzung oder Ticket suchen …", "Search command, session or ticket …")
                        } else {
                            lang.t("Befehl oder Sitzung suchen …", "Search command or session …")
                        }),
                );
                te.request_focus();
                if p.query != before {
                    p.sel = 0;
                }
                ui.add_space(4.0);
                for (i, (_, label, hint, act)) in items.iter().enumerate() {
                    let r = ui.horizontal(|ui| {
                        let resp = ui.selectable_label(i == p.sel, label);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.weak(hint);
                        });
                        resp
                    });
                    if r.inner.clicked() {
                        chosen = Some(act.clone());
                    }
                }
                if items.is_empty() {
                    ui.weak(lang.t("Keine Treffer", "No matches"));
                }
            });
        if chosen.is_some() {
            close = true;
        }
        if let Some(a) = chosen {
            actions.push(a);
        }
        if close {
            self.want_focus = Some(self.focused);
        } else {
            self.palette = Some(p);
        }
    }
}
