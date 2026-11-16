//! Dialoge und kleine Fenster.

use super::*;

impl App {
    pub(super) fn dialogs(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let lang = self.settings.language;

        // Neue Sitzung
        if let Some(mut d) = self.new_dialog.take() {
            let mut keep = true;
            let mut create = false;
            egui::Window::new(if d.claude { lang.t("Neue Claude-Sitzung", "New Claude session") } else { lang.t("Neue Sitzung", "New session") })
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ctx, |ui| {
                    egui::Grid::new("newgrid").num_columns(2).spacing([8.0, 8.0]).show(ui, |ui| {
                        if !self.settings.profiles.is_empty() {
                            ui.label(lang.t("Profil", "Profile"));
                            let cur = self.settings.profiles.iter().find(|p| p.id == d.profile).map(|p| p.name.clone()).unwrap_or_else(|| lang.t("Keins", "None").to_string());
                            egui::ComboBox::from_id_salt("newprof").selected_text(cur).show_ui(ui, |ui| {
                                if ui.selectable_label(d.profile == 0, lang.t("Keins", "None")).clicked() {
                                    d.profile = 0;
                                }
                                for p in &self.settings.profiles {
                                    if ui.selectable_label(d.profile == p.id, &p.name).clicked() {
                                        d.profile = p.id;
                                        if !p.shell_id.is_empty() {
                                            d.shell_id = p.shell_id.clone();
                                        }
                                        if !p.cwd.is_empty() {
                                            d.cwd = p.cwd.clone();
                                        }
                                    }
                                }
                            });
                            ui.end_row();
                        }
                        ui.label("Shell");
                        let cur = self.shells.iter().find(|s| s.id == d.shell_id).map(|s| s.label.clone()).unwrap_or_default();
                        egui::ComboBox::from_id_salt("shellsel").selected_text(cur).show_ui(ui, |ui| {
                            for s in &self.shells {
                                ui.selectable_value(&mut d.shell_id, s.id.clone(), &s.label);
                            }
                        });
                        ui.end_row();
                        ui.label(lang.t("Ordner", "Folder"));
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut d.cwd).desired_width(290.0));
                            if ui.button("…").on_hover_text(lang.t("Ordner wählen", "Choose folder")).clicked() {
                                if let Some(p) = notify::pick_folder(&d.cwd) {
                                    d.cwd = p;
                                }
                            }
                        });
                        ui.end_row();
                        ui.label(lang.t("Name (optional)", "Name (optional)"));
                        ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(320.0));
                        ui.end_row();
                        ui.label(lang.t("Ordner im Baum", "Tree folder"));
                        ui.add(egui::TextEdit::singleline(&mut d.group).hint_text(lang.t("z. B. Backend/API (leer = keiner)", "e.g. Backend/API (empty = none)")).desired_width(320.0));
                        ui.end_row();
                        if self.settings.claude_mode {
                            ui.label("");
                            ui.checkbox(&mut d.claude, lang.t("Claude Code starten", "Start Claude Code"));
                            ui.end_row();
                            if d.claude {
                                ui.label(lang.t("Modell", "Model"));
                                ui.horizontal(|ui| {
                                    ui.add(egui::TextEdit::singleline(&mut d.model).hint_text("z.B. opus, sonnet, haiku").desired_width(200.0));
                                    ui.menu_button("▾", |ui| {
                                        for m in ["", "opus", "sonnet", "haiku"] {
                                            if ui.button(if m.is_empty() { lang.t("Standard", "Default") } else { m }).clicked() {
                                                d.model = m.to_string();
                                                ui.close();
                                            }
                                        }
                                    });
                                });
                                ui.end_row();
                                ui.label("Permission-Mode");
                                egui::ComboBox::from_id_salt("permsel").selected_text(PERMS[d.perm].1).show_ui(ui, |ui| {
                                    for (i, (_, label)) in PERMS.iter().enumerate() {
                                        ui.selectable_value(&mut d.perm, i, *label);
                                    }
                                });
                                ui.end_row();
                                ui.label("");
                                ui.checkbox(&mut d.continue_last, lang.t("Letzte Konversation im Ordner fortsetzen (--continue)", "Continue last conversation in folder (--continue)"));
                                ui.end_row();
                                ui.label("");
                                session_settings::continue_checkbox(ui, &mut d.claude_continue, lang);
                                ui.end_row();
                                ui.label(lang.t("Start-Prompt", "Start prompt"));
                                ui.add(egui::TextEdit::singleline(&mut d.prompt).desired_width(320.0));
                                ui.end_row();
                                // Git-Worktree anbieten, wenn der Ordner in einem Repository liegt (Prüfung nur bei Änderung des Pfads,
                                // ohne git-Prozess pro Tastendruck; das echte Wurzelverzeichnis ermittelt erst das Anlegen).
                                if d.repo_for != d.cwd {
                                    d.repo_for = d.cwd.clone();
                                    let p = std::path::Path::new(&d.cwd);
                                    d.repo = gitops::inside_repo_fs(p).then(|| p.to_path_buf());
                                }
                                if d.repo.is_some() {
                                    ui.label("Git");
                                    ui.horizontal(|ui| {
                                        if ui.checkbox(&mut d.worktree, lang.t("Eigener Worktree", "Own worktree")).changed() && d.branch.is_empty() {
                                            let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|x| x.as_secs() % 100000).unwrap_or(0);
                                            d.branch = format!("claude/{secs}");
                                        }
                                        if d.worktree {
                                            ui.add(egui::TextEdit::singleline(&mut d.branch).desired_width(180.0).hint_text("Branch"));
                                        }
                                    });
                                    ui.end_row();
                                    if d.worktree {
                                        ui.label("");
                                        ui.label(RichText::new(lang.t("Legt einen neuen Branch und Ordner neben dem Repo an, damit parallele Sitzungen sich nicht stören.", "Creates a new branch and folder next to the repo so parallel sessions don't interfere.")).small().color(style::dim(ui)));
                                        ui.end_row();
                                    }
                                }
                            }
                        }
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button(lang.t("Erstellen", "Create")).clicked() {
                            create = true;
                        }
                        if ui.button(lang.t("Abbrechen", "Cancel")).clicked() {
                            keep = false;
                        }
                    });
                });
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                keep = false;
            }
            if create {
                let mut args = Vec::new();
                if !d.model.trim().is_empty() {
                    args.push(format!("--model {}", d.model.trim().replace(' ', "")));
                }
                if d.perm > 0 {
                    args.push(format!("--permission-mode {}", PERMS[d.perm].0));
                }
                let opts = NewOpts {
                    shell_id: Some(d.shell_id.clone()),
                    cwd: Some(d.cwd.clone()),
                    name: Some(d.name.clone()),
                    claude: d.claude,
                    claude_args: args.join(" "),
                    continue_last: d.continue_last,
                    claude_continue: Some(d.claude_continue),
                    prompt: Some(d.prompt.clone()),
                    profile: Some(d.profile),
                    group: Some(d.group.clone()),
                    ..NewOpts::default()
                };
                'create: {
                    let want_wt = d.worktree && d.claude && d.repo.is_some() && !d.branch.trim().is_empty();
                    if want_wt && self.pending_wt.is_some() {
                        // Sonst ginge die laufende Anlage verloren (Worktree auf der Platte, aber keine Sitzung).
                        self.notify_text(self.settings.language.t("Es wird noch ein Worktree angelegt – bitte kurz warten.", "A worktree is still being created – please wait.").to_string());
                        self.new_dialog = Some(d);
                        break 'create;
                    }
                    let repo = if want_wt { gitops::repo_root(std::path::Path::new(&d.cwd)) } else { None };
                    if want_wt && repo.is_none() {
                        self.notify_text(self.settings.language.t("Kein Git-Repository gefunden.", "No Git repository found.").to_string());
                        self.new_dialog = Some(d);
                        break 'create;
                    }
                    match (want_wt, repo) {
                        (true, Some(repo)) => {
                            let branch = d.branch.trim().to_string();
                            let path = gitops::worktree_path(&repo, &branch);
                            let (tx, rx) = std::sync::mpsc::channel();
                            let (r2, b2, p2) = (repo.clone(), branch.clone(), path.clone());
                            let ctx2 = ctx.clone();
                            std::thread::spawn(move || {
                                let _ = tx.send(gitops::worktree_add(&r2, &b2, &p2));
                                ctx2.request_repaint();
                            });
                            let mut opts = opts;
                            opts.cwd = Some(path.to_string_lossy().to_string());
                            // Fertig wird die Anlage im Hauptbild: Fenster und Projekt des Dialogs mitnehmen (#110).
                            opts.window = Some(self.wins.cur);
                            opts.project = Some(self.active_project);
                            self.pending_wt = Some(PendingWt {
                                rx,
                                opts,
                                wt: crate::state::Worktree { repo: repo.to_string_lossy().to_string(), path: path.to_string_lossy().to_string(), branch },
                            });
                            self.notify_text(self.settings.language.t("Worktree wird angelegt …", "Creating worktree …").to_string());
                        }
                        _ => {
                            self.new_session(ctx, opts);
                        }
                    }
                }
            } else if keep {
                self.new_dialog = Some(d);
            }
        }

        // Gruppe benennen
        if let Some(mut p) = self.group_prompt.take() {
            let mut keep = true;
            let mut ok = false;
            egui::Window::new(lang.t("Neuer Ordner für die Sitzung (z. B. Backend/API)", "New folder for the session (e.g. Backend/API)"))
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ctx, |ui| {
                    let te = ui.text_edit_singleline(&mut p.text);
                    if !te.has_focus() {
                        te.request_focus();
                    }
                    ui.horizontal(|ui| {
                        if ui.button("OK").clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                            ok = true;
                        }
                        if ui.button(lang.t("Abbrechen", "Cancel")).clicked() {
                            keep = false;
                        }
                    });
                });
            if ok {
                actions.push(Action::SetGroup(p.id, p.text.trim().to_string()));
            } else if keep {
                self.group_prompt = Some(p);
            }
        }

        // Ordner anlegen / umbenennen / Prompt an alle
        if let Some(mut fp) = self.folder_prompt.take() {
            let mut keep = true;
            let mut ok = false;
            let title = match fp.kind {
                0 => lang.t("Neuer Ordner", "New folder"),
                1 => lang.t("Ordner umbenennen", "Rename folder"),
                3 => lang.t("Als Vorlage speichern – Name", "Save as template – name"),
                _ => lang.t("Prompt an alle Sitzungen im Ordner", "Prompt to all sessions in folder"),
            };
            egui::Window::new(title).collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
                if fp.kind == 2 {
                    let n = self.folder_sessions(&fp.path).len();
                    ui.label(RichText::new(format!("{} {}", n, lang.t("Sitzungen", "sessions"))).small().color(style::dim(ui)));
                    ui.add(egui::TextEdit::multiline(&mut fp.text).desired_rows(4).desired_width(360.0));
                } else {
                    if fp.kind == 0 && !fp.path.is_empty() {
                        ui.label(RichText::new(format!("{}: {}", lang.t("In", "In"), fp.path)).small().color(style::dim(ui)));
                    }
                    let te = ui.text_edit_singleline(&mut fp.text);
                    if !te.has_focus() {
                        te.request_focus();
                    }
                }
                ui.horizontal(|ui| {
                    let label = if fp.kind == 2 { lang.t("Senden", "Send") } else { "OK" };
                    if ui.button(label).clicked() || (fp.kind != 2 && ui.input(|i| i.key_pressed(Key::Enter))) {
                        ok = true;
                    }
                    if ui.button(lang.t("Abbrechen", "Cancel")).clicked() {
                        keep = false;
                    }
                });
            });
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                keep = false;
            }
            if ok {
                let pid = self.active_project;
                match fp.kind {
                    0 => {
                        let leaf = state::clean_folder_path(&fp.text);
                        if !leaf.is_empty() {
                            let full = if fp.path.is_empty() { leaf } else { format!("{}/{}", fp.path, leaf) };
                            if let Some(p) = self.projects.iter_mut().find(|p| p.id == pid) {
                                p.ensure_folder(&full);
                            }
                        }
                    }
                    1 => {
                        let leaf = state::clean_folder_path(&fp.text).replace('/', "-");
                        let parent = state::folder_parent(&fp.path).to_string();
                        let new = if parent.is_empty() || leaf.is_empty() { leaf } else { format!("{parent}/{leaf}") };
                        let _ = pid;
                        if new.is_empty() || !self.rename_folder_to(&fp.path, &new) {
                            self.notify_text(lang.t("Name ungültig oder schon vergeben", "Name invalid or already taken").to_string());
                        }
                    }
                    3 => {
                        let (n, f) = (fp.text.clone(), fp.path.clone());
                        self.apply(ctx, Action::TemplateSave(n, Some(f)));
                    }
                    _ => {
                        let text = fp.text.trim();
                        if !text.is_empty() {
                            for id in self.folder_sessions(&fp.path) {
                                if let Some(s) = self.entries.get(&id).and_then(|e| e.live.as_ref()).filter(|s| !s.has_exited()) {
                                    s.send_text(text, true);
                                }
                            }
                        }
                    }
                }
            } else if keep {
                self.folder_prompt = Some(fp);
            }
        }

        // Schließen bestätigen (eine oder viele Sitzungen, mit 3-Sekunden-Wartezeit)
        self.close_ask_ui(ctx, actions);
        // Einstellungen einer Sitzung und Rückfrage „Neu laden?“ (#169, #170).
        // Die Rückfrage zuerst: sie bekommt Esc/Enter, bevor der Einstellungsdialog darunter sie sieht.
        self.reload_ask_ui(ctx, actions, None);
        self.session_settings_ui(ctx);

        self.history_ui(ctx, actions);
        self.templates_ui(ctx, actions);
        self.costs_ui(ctx);
        self.gsearch_ui(ctx, actions);
        self.queue_ui(ctx);
        self.composer_ui(ctx, actions);
        self.diff_ui(ctx);
        self.wt_close_ui(ctx);
        actions.append(&mut self.pending_actions);
        self.editor_ui(ctx);
        self.palette_ui(ctx, actions);
        self.prompts_ui(ctx, actions);
        self.notes_ui(ctx);

        self.new_chat_ui(ctx);
        self.settings_ui(ctx);
    }

    pub(super) fn text_prompt_ui(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(mut p) = self.text_prompt.take() else { return };
        let lang = self.settings.language;
        let title = if p.tags_for.is_some() {
            lang.t("Tags (mit Komma getrennt)", "Tags (comma separated)")
        } else {
            match p.kind {
                PromptKind::NewProject => lang.t("Neues Projekt", "New project"),
                PromptKind::RenameProject => lang.t("Projekt umbenennen", "Rename project"),
                PromptKind::ProjectFolder => lang.t("Standardordner des Projekts", "Project default folder"),
            }
        };
        let (mut ok, mut cancel) = (false, ctx.input(|i| i.key_pressed(Key::Escape)));
        egui::Window::new(title).collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
            let te = ui.add(egui::TextEdit::singleline(&mut p.text).desired_width(340.0));
            if !te.has_focus() {
                te.request_focus();
            }
            ui.horizontal(|ui| {
                if ui.button("OK").clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                    ok = true;
                }
                if ui.button(lang.t("Abbrechen", "Cancel")).clicked() {
                    cancel = true;
                }
            });
        });
        if ok {
            let text = p.text.trim().to_string();
            if let Some(sid) = p.tags_for {
                if let Some(e) = self.entries.get_mut(&sid) {
                    e.meta.tags = text.split(',').map(|t| t.trim().trim_start_matches('#').to_string()).filter(|t| !t.is_empty()).collect();
                }
            } else {
                match p.kind {
                    PromptKind::NewProject => {
                        if !text.is_empty() {
                            let id = self.projects.iter().map(|p| p.id).max().unwrap_or(0) + 1;
                            let color = PALETTE[(id as usize) % PALETTE.len()];
                            self.projects.push(Project { id, name: text, color, ..Project::default() });
                            actions.push(Action::SwitchProject(id));
                        }
                    }
                    PromptKind::RenameProject => {
                        if let Some(pr) = self.projects.iter_mut().find(|x| x.id == p.id) {
                            if !text.is_empty() {
                                pr.name = text;
                            }
                        }
                    }
                    PromptKind::ProjectFolder => {
                        if let Some(pr) = self.projects.iter_mut().find(|x| x.id == p.id) {
                            pr.folder = text;
                        }
                    }
                }
            }
        } else if !cancel {
            self.text_prompt = Some(p);
        }
    }

    pub(super) fn templates_ui(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        if !self.templates_open {
            return;
        }
        let lang = self.settings.language;
        let mut open = true;
        let mut changed = false;
        egui::Window::new(lang.t("Sitzungs-Vorlagen", "Session templates")).open(&mut open).default_width(480.0).show(ctx, |ui| {
            ui.label(RichText::new(lang.t("Eine Vorlage startet mehrere Sitzungen mit Ordner, Profil, Claude-Optionen und (optional) als Raster – mit einem Klick.", "A template starts several sessions with folder, profile, Claude options and (optionally) as a grid – in one click.")).small().color(style::dim(ui)));
            ui.add_space(S1);
            if self.templates.is_empty() {
                ui.weak(lang.t("Noch keine Vorlagen.", "No templates yet."));
            }
            for i in 0..self.templates.len() {
                let t = &mut self.templates[i];
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&t.name).strong());
                        ui.label(RichText::new(t.items.iter().map(|it| it.name.as_str()).collect::<Vec<_>>().join(", ")).small().color(style::dim(ui)));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(lang.t("Starten", "Start")).clicked() {
                            actions.push(Action::TemplateStart(i));
                        }
                        if armed_button(ui, ("tpldel", i), "🗑", lang.t("Wirklich?", "Sure?")) {
                            actions.push(Action::TemplateDelete(i));
                        }
                        if ui.checkbox(&mut t.grid, lang.t("Raster", "Grid")).changed() {
                            changed = true;
                        }
                    });
                });
                ui.separator();
            }
            ui.label(lang.t("Alle Sitzungen dieses Projekts als Vorlage speichern:", "Save all sessions of this project as a template:"));
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.tpl_name).hint_text(lang.t("Name der Vorlage", "Template name")).desired_width(240.0));
                if ui.button(lang.t("Speichern", "Save")).clicked() {
                    actions.push(Action::TemplateSave(std::mem::take(&mut self.tpl_name), None));
                }
            });
            ui.label(RichText::new(lang.t("Tipp: Einzelne Ordner speicherst du per Rechtsklick → „Als Vorlage speichern“. Gleicher Name überschreibt.", "Tip: save single folders via right-click → “Save as template”. Same name overwrites.")).small().color(style::dim(ui)));
        });
        if changed {
            templates::save(&self.dir, &self.templates);
        }
        // Nicht verschachteln: ctx.input() hält die Schreibsperre des Kontexts, wants_keyboard_input() bräuchte
        // eine Lesesperre derselben (nicht reentranten) Sperre – das hängte die App beim Drücken von Esc auf.
        let esc = ctx.input(|i| i.key_pressed(Key::Escape));
        self.templates_open = open && !(esc && !ctx.wants_keyboard_input());
    }

    pub(super) fn wt_close_ui(&mut self, ctx: &egui::Context) {
        let lang = self.settings.language;
        let Some(mut w) = self.wt_close.take() else { return };
        let Some(wt) = self.entries.get(&w.id).and_then(|e| e.meta.worktree.clone()) else { return };
        let name = self.entries.get(&w.id).map(|e| e.display_name()).unwrap_or_default();
        let mut keep = true;
        let mut action: Option<u8> = None; // 0 behalten, 1 entfernen, 2 mergen
        egui::Window::new(lang.t("Sitzung mit Worktree schließen", "Close session with worktree"))
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(lang.q(&name));
                ui.label(RichText::new(format!("{}: {}\n{}: {}", lang.t("Branch", "Branch"), wt.branch, lang.t("Ordner", "Folder"), wt.path)).small().color(style::dim(ui)));
                ui.add_space(S2);
                if w.changes > 0 {
                    ui.colored_label(Color32::from_rgb(0xe5, 0xc0, 0x7b), format!("{} {}", w.changes, lang.t("Datei(en) mit nicht committeten Änderungen.", "file(s) with uncommitted changes.")));
                    ui.checkbox(&mut w.discard, lang.t("Änderungen verwerfen (Entfernen erzwingen)", "Discard changes (force removal)"));
                } else {
                    ui.label(RichText::new(lang.t("Keine offenen Änderungen.", "No pending changes.")).color(style::dim(ui)));
                }
                ui.add_space(S2);
                ui.horizontal(|ui| {
                    if ui.button(lang.t("Behalten", "Keep")).on_hover_text(lang.t("Sitzung schließen, Worktree und Branch bleiben", "Close the session, keep worktree and branch")).clicked() {
                        action = Some(0);
                    }
                    if ui.add_enabled(w.changes == 0 || w.discard, egui::Button::new(lang.t("Entfernen", "Remove"))).on_hover_text(lang.t("Worktree und Branch löschen", "Delete worktree and branch")).clicked() {
                        action = Some(1);
                    }
                    if ui.add_enabled(w.changes == 0, egui::Button::new(lang.t("Mergen & entfernen", "Merge & remove"))).on_hover_text(lang.t("Branch in den aktuellen Branch des Haupt-Repos mergen, dann aufräumen (bei Konflikt wird abgebrochen)", "Merge the branch into the main repo's current branch, then clean up (aborts on conflict)")).clicked() {
                        action = Some(2);
                    }
                    if ui.button(lang.t("Abbrechen", "Cancel")).clicked() {
                        keep = false;
                    }
                });
            });
        if let Some(a) = action {
            let id = w.id;
            self.close_session(id);
            if a > 0 {
                let (repo, path, branch, force) = (PathBuf::from(&wt.repo), PathBuf::from(&wt.path), wt.branch.clone(), w.discard);
                let ok = lang.t("Worktree aufgeräumt", "Worktree cleaned up");
                self.git_bg(ctx, ok, move || {
                    std::thread::sleep(Duration::from_millis(800)); // Prozess beenden lassen, damit der Ordner frei wird
                    if a == 2 {
                        gitops::merge(&repo, &branch).map_err(|e| format!("{}: {e}", lang.t("Merge fehlgeschlagen (abgebrochen)", "Merge failed (aborted)")))?;
                    }
                    gitops::worktree_remove(&repo, &path, force)?;
                    if a == 2 {
                        gitops::branch_delete(&repo, &branch, true)?;
                    } else if gitops::branch_delete(&repo, &branch, false).is_err() {
                        // „-d“ statt „-D“: Commits, die noch nirgends gemergt sind, nicht still verwerfen.
                        return Err(format!("{} {}", lang.t("Worktree entfernt; Branch mit nicht gemergten Commits wurde behalten:", "Worktree removed; branch with unmerged commits was kept:"), lang.q(&branch)));
                    }
                    Ok(String::new())
                });
            }
        } else if keep {
            self.wt_close = Some(w);
        }
    }

    pub(super) fn composer_ui(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let lang = self.settings.language;
        if !self.composer_open {
            return;
        }
        let target = self.entries.get(&self.focused).map(|e| e.display_name()).unwrap_or_default();
        let esc_close = ctx.input(|i| i.key_pressed(Key::Escape));
        let mut open = true;
        let ctrl_enter = ctx.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::Enter));
        let mut send = ctrl_enter;
        let (mut paste_only, mut to_queue, mut to_all, mut save_prompt) = (false, false, false, false);
        egui::Window::new(lang.t("Prompt-Composer", "Prompt composer")).open(&mut open).default_size([680.0, 420.0]).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{}: {}", lang.t("Ziel", "Target"), target)).strong());
                ui.label(RichText::new(format!("{} {} · {} {}", self.composer_text.lines().count(), lang.t("Zeilen", "lines"), self.composer_text.chars().count(), lang.t("Zeichen", "characters"))).small().color(style::dim(ui)));
            });
            ui.add_space(S1);
            let te = ui.add(
                egui::TextEdit::multiline(&mut self.composer_text)
                    .desired_rows(14)
                    .desired_width(f32::INFINITY)
                    .lock_focus(true)
                    .hint_text(lang.t("Langen Prompt schreiben … Strg+Enter sendet", "Write a long prompt … Ctrl+Enter sends")),
            );
            if self.composer_focus {
                te.request_focus();
                self.composer_focus = false;
            }
            ui.add_space(S1);
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(!self.composer_text.trim().is_empty(), egui::Button::new(RichText::new(lang.t("Senden (Strg+Enter)", "Send (Ctrl+Enter)")).strong()).fill(style::ACCENT.gamma_multiply(0.7))).clicked() {
                    send = true;
                }
                if ui.add_enabled(!self.composer_text.trim().is_empty(), egui::Button::new(lang.t("Nur einfügen", "Insert only"))).on_hover_text(lang.t("Ohne Enter – zum Weiterbearbeiten im Terminal", "Without Enter – to keep editing in the terminal")).clicked() {
                    paste_only = true;
                }
                if self.settings.claude_mode && ui.add_enabled(!self.composer_text.trim().is_empty(), egui::Button::new(lang.t("In Warteschlange", "Add to queue"))).clicked() {
                    to_queue = true;
                }
                if ui.add_enabled(!self.composer_text.trim().is_empty(), egui::Button::new(lang.t("An alle sichtbaren", "To all visible"))).clicked() {
                    to_all = true;
                }
                if ui.add_enabled(!self.composer_text.trim().is_empty(), egui::Button::new(lang.t("Als Prompt speichern", "Save as prompt"))).clicked() {
                    save_prompt = true;
                }
                ui.checkbox(&mut self.composer_keep, lang.t("Text nach dem Senden behalten", "Keep text after sending"));
            });
        });
        let text = self.composer_text.clone();
        let has = !text.trim().is_empty();
        if has && (send || paste_only) {
            // Nur wenn wirklich gesendet wurde: ohne laufende Sitzung bliebe der (lange) Text sonst nicht erhalten.
            // Chat im Fokus: Text landet in dessen Eingabefeld (abgeschickt wird dort).
            let sent = self.entries.get_mut(&self.focused).is_some_and(|e| e.insert_text(&text, send));
            if sent && !self.composer_keep {
                self.composer_text.clear();
            }
            if sent && send {
                open = false;
            }
            if !sent {
                self.notify_text(lang.t("Keine laufende Sitzung im Fokus – Text bleibt erhalten", "No running session focused – text kept").to_string());
            }
        }
        if has && to_queue {
            actions.push(Action::QueueAdd(self.focused, text.clone()));
            if !self.composer_keep {
                self.composer_text.clear();
            }
        }
        if has && to_all {
            let mut leaves = Vec::new();
            if let Some(l) = &self.layout {
                l.leaves(&mut leaves);
            }
            let mut sent = 0;
            for id in leaves {
                if let Some(s) = self.entries.get(&id).and_then(|e| e.live.as_ref()) {
                    s.send_text(&text, true);
                    sent += 1;
                }
            }
            if sent > 0 {
                if !self.composer_keep {
                    self.composer_text.clear();
                }
                open = false;
            }
        }
        if has && save_prompt {
            let name: String = text.lines().next().unwrap_or("Prompt").chars().take(32).collect();
            self.prompts.push(Snippet { name, text: text.clone(), submit: false });
            state::save_json(&self.dir.join("prompts.json"), &self.prompts);
            self.notify_text(lang.t("In der Prompt-Bibliothek gespeichert", "Saved to the prompt library").to_string());
        }
        if !open || (esc_close && !ctx.wants_keyboard_input()) {
            self.composer_open = false;
            self.want_focus = Some(self.focused);
        }
    }

    pub(super) fn prompts_ui(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        if !self.prompts_open {
            return;
        }
        let lang = self.settings.language;
        let mut open = true;
        egui::Window::new(lang.t("Prompt-Bibliothek", "Prompt library"))
            .open(&mut open)
            .default_size([640.0, 360.0])
            .show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(190.0);
                        egui::ScrollArea::vertical().max_height(300.0).id_salt("plist").show(ui, |ui| {
                            for (i, s) in self.prompts.iter().enumerate() {
                                let name = if s.name.is_empty() { "—" } else { &s.name };
                                if ui.selectable_label(self.prompt_sel == i, name).clicked() {
                                    self.prompt_sel = i;
                                }
                            }
                        });
                        if ui.button(lang.t("+ Neuer Prompt", "+ New prompt")).clicked() {
                            self.prompts.push(Snippet { name: lang.t("Neuer Prompt", "New prompt").into(), ..Snippet::default() });
                            self.prompt_sel = self.prompts.len() - 1;
                        }
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        if let Some(s) = self.prompts.get_mut(self.prompt_sel) {
                            let qtext = s.text.clone();
                            ui.add(egui::TextEdit::singleline(&mut s.name).hint_text("Name").desired_width(f32::INFINITY));
                            ui.add(egui::TextEdit::multiline(&mut s.text).desired_rows(9).desired_width(f32::INFINITY).hint_text("Prompt …"));
                            ui.checkbox(&mut s.submit, lang.t("Nach dem Einfügen automatisch Enter senden", "Press Enter automatically after inserting"));
                            ui.horizontal(|ui| {
                                if ui.button(lang.t("An fokussiertes Terminal senden", "Send to focused terminal")).clicked() {
                                    actions.push(Action::SendSnippet(self.prompt_sel));
                                }
                                if self.settings.claude_mode && ui.button(lang.t("In Warteschlange", "Add to queue")).clicked() {
                                    actions.push(Action::QueueAdd(self.focused, qtext.clone()));
                                }
                                if ui.button(lang.t("Löschen", "Delete")).clicked() {
                                    self.prompts.remove(self.prompt_sel);
                                    self.prompt_sel = self.prompt_sel.saturating_sub(1);
                                }
                            });
                        } else {
                            ui.weak(lang.t("Noch keine Prompts.", "No prompts yet."));
                        }
                    });
                });
            });
        if !open {
            self.prompts_open = false;
            state::save_json(&self.dir.join("prompts.json"), &self.prompts);
        }
    }

    pub(super) fn notes_ui(&mut self, ctx: &egui::Context) {
        let Some(id) = self.notes_open else { return };
        let lang = self.settings.language;
        let Some(e) = self.entries.get_mut(&id) else {
            self.notes_open = None;
            return;
        };
        let mut open = true;
        egui::Window::new(format!("{} – {}", lang.t("Notizen", "Notes"), e.display_name()))
            .id(Id::new("notes_win"))
            .open(&mut open)
            .default_size([340.0, 220.0])
            .show(ctx, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut e.meta.notes)
                        .desired_width(f32::INFINITY)
                        .desired_rows(8)
                        .hint_text(lang.t("z.B. „Warte auf Review von …“", "e.g. “Waiting for review from …”")),
                );
            });
        if !open {
            self.notes_open = None;
        }
    }
}
