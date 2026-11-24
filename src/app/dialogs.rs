//! Dialoge und kleine Fenster.

use super::*;

impl App {
    pub(super) fn dialogs(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let lang = self.settings.language;
        // Rückfrage „Link öffnen?“ (OSC 8, #183)
        self.link_ask_ui(ctx, self.focused);

        // Neue Sitzung (Inhalt in `new_dialog_ui`, ohne App testbar)
        if let Some(mut d) = self.new_dialog.take() {
            if d.focus {
                // Beim Öffnen: vorhandene Tags für die Vorschläge (#239, #292).
                d.tag_values = self.session_tag_counts();
            }
            let target = self.new_dialog_target(&d.shell_id, d.profile);
            let spec = self.preview_spec(&d.shell_id, d.profile);
            ctx.data_mut(|m| m.insert_temp(preview_target_id("newdlg"), (spec, String::new())));
            let out = new_dialog_ui(ctx, &mut d, &self.shells, &self.settings.profiles, self.settings.claude_mode, &self.settings.wsl_recent, target, lang);
            let create = out == NewDlgOut::Create;
            let keep = out == NewDlgOut::Open;
            if out == NewDlgOut::Cancel {
                // Fokus zurück ins Terminal, aus dem der Dialog kam.
                self.want_focus = Some(self.focused);
            }
            if create && d.claude {
                // Letzte Wahl merken (Vorauswahl beim nächsten Mal) – nie bypassPermissions.
                let copts = d.copts.sanitized(false);
                if self.settings.last_claude_opts.as_ref() != Some(&copts) {
                    self.settings.last_claude_opts = Some(copts);
                    state::save_json(&self.dir.join("settings.json"), &self.settings);
                }
            }
            if create {
                if d.picker.is_wsl() {
                    d.picker.remember(&mut self.settings.wsl_recent);
                    state::save_json(&self.dir.join("settings.json"), &self.settings);
                }
                let opts = new_dialog_opts(&d);
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
            style::movable(ctx, Id::new("group_prompt"), egui::Window::new(lang.t("Neuer Ordner für die Sitzung (z. B. Backend/API)", "New folder for the session (e.g. Backend/API)")))
                .collapsible(false)
                .resizable(false)
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
            style::movable(ctx, Id::new("folder_prompt"), egui::Window::new(title)).collapsible(false).resizable(false).show(ctx, |ui| {
                if fp.kind == 2 {
                    let n = self.folder_sessions(&fp.path).len();
                    ui.label(RichText::new(format!("{} {}", n, lang.t("Sitzungen", "sessions"))).small().color(style::dim(ui)));
                    let te = ui.add(egui::TextEdit::multiline(&mut fp.text).desired_rows(4).desired_width(360.0));
                    // Tasten gehören dem Dialog, nicht dem Terminal dahinter (#255).
                    if !te.has_focus() {
                        te.request_focus();
                    }
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
            // Esc verbrauchen: es erreicht kein Terminal dahinter (#255).
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
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
        // Prozessliste (#243): Bearbeiten-Dialog und Rückfrage beim Beenden, nur im Hauptfenster.
        if self.wins.cur == 0 {
            self.procs_dialogs_ui(ctx);
        }
        // Einstellungen einer Sitzung und Rückfrage „Neu laden?“ (#169, #170).
        // Die Rückfrage zuerst: sie bekommt Esc/Enter, bevor der Einstellungsdialog darunter sie sieht.
        self.reload_ask_ui(ctx, actions, None);
        // Auswahl der Konversation vor einem Start (#302): nur im Hauptfenster, eine zur Zeit.
        if self.wins.cur == 0 {
            self.conv_pick_ui(ctx);
        }
        // Rückfrage vor dem Einfügen mehrerer Zeilen (#183).
        self.paste_ask_ui(ctx, None);
        self.session_settings_ui(ctx);

        self.history_ui(ctx, actions);
        self.templates_ui(ctx, actions);
        self.tpl_editor_ui(ctx);
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
        self.theme_ui_frame(ctx);
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
                PromptKind::FolderCwd => lang.t("Arbeitsordner", "Working folder"),
            }
        };
        // Esc bei offener Vorschlagsliste schließt nur die Liste.
        let (mut ok, mut cancel) = (false, ctx.input(|i| i.key_pressed(Key::Escape)) && !egui::Popup::is_any_open(ctx));
        style::movable(ctx, Id::new("text_prompt"), egui::Window::new(title)).collapsible(false).resizable(false).show(ctx, |ui| {
            let blocked = match &mut p.picker {
                // Ordnervorgabe mit WSL-Shell: Ordnerfeld mit Linux-Pfad (#172).
                Some(picker) => {
                    picker.show(ui, lang, &self.settings.wsl_recent);
                    picker.blocking_error(lang)
                }
                None => {
                    let te = match p.tags_for {
                        // Tags: Vorschläge aus allen Sitzungen mit Anzahl (#239)
                        Some(sid) => {
                            let id = egui::Id::new(("tags_prompt", sid));
                            let used = super::planning_ui::value_suggest::cached_per_open(ctx, id, || self.session_tag_counts());
                            super::planning_ui::value_suggest::suggest_edit(ui, id, &mut p.text, &used, &[], super::planning_ui::value_suggest::Mode::List, lang, |ui, te| ui.add(te.desired_width(340.0))).response
                        }
                        None => ui.add(egui::TextEdit::singleline(&mut p.text).desired_width(340.0)),
                    };
                    if !te.has_focus() {
                        te.request_focus();
                    }
                    None
                }
            };
            ui.horizontal(|ui| {
                let b = ui.add_enabled(blocked.is_none(), egui::Button::new("OK"));
                if b.clicked() || (blocked.is_none() && ui.input(|i| i.key_pressed(Key::Enter))) {
                    ok = true;
                }
                if let Some(e) = &blocked {
                    b.on_disabled_hover_text(e.as_str());
                }
                if ui.button(lang.t("Abbrechen", "Cancel")).clicked() {
                    cancel = true;
                }
            });
        });
        if ok {
            let text = match &p.picker {
                Some(picker) => {
                    if picker.is_wsl() {
                        picker.remember(&mut self.settings.wsl_recent);
                        state::save_json(&self.dir.join("settings.json"), &self.settings);
                    }
                    picker.value()
                }
                None => p.text.trim().to_string(),
            };
            if let Some(sid) = p.tags_for {
                if let Some(e) = self.entries.get_mut(&sid) {
                    e.meta.tags = text.split(',').map(|t| t.trim().trim_start_matches('#').to_string()).filter(|t| !t.is_empty()).collect();
                }
            } else {
                match p.kind {
                    PromptKind::NewProject => {
                        if !text.is_empty() {
                            let id = self.projects.iter().map(|p| p.id).max().unwrap_or(0) + 1;
                            let color = hub_color(id);
                            // Neue Projekte beginnen für Agenten gesperrt (#386).
                            self.projects.push(Project { id, name: text, color, agent_access: crate::access::AgentAccess::LOCKED, ..Project::default() });
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
                    PromptKind::FolderCwd => {
                        if let Some(f) = self.projects.iter_mut().find(|x| x.id == p.id).and_then(|pr| pr.folders.iter_mut().find(|f| f.path == p.folder)) {
                            f.cwd = text;
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
        style::movable(ctx, Id::new("templates_win"), egui::Window::new(lang.t("Sitzungs-Vorlagen", "Session templates"))).open(&mut open).default_width(480.0).show(ctx, |ui| {
            ui.label(RichText::new(lang.t("Eine Gruppen-Vorlage startet mehrere Sitzungen mit Ordner, Profil, Claude-Optionen und (optional) als Raster – mit einem Klick. Eine Terminal-Vorlage startet eine Sitzung mit allen Einstellungen. Alle Vorlagen stehen auch unten in der Seitenleiste.", "A group template starts several sessions with folder, profile, Claude options and (optionally) as a grid – in one click. A terminal template starts one session with all its settings. All templates are also at the bottom of the sidebar.")).small().color(style::dim(ui)));
            ui.add_space(S1);
            if self.templates.is_empty() {
                ui.weak(lang.t("Noch keine Vorlagen.", "No templates yet."));
            }
            // Viele Vorlagen: Liste scrollt, Speichern bleibt sichtbar (#241).
            let list_h = (dialog_body_height(ctx) - 110.0).max(100.0);
            egui::ScrollArea::vertical().id_salt("tpl_list").max_height(list_h).min_scrolled_height(list_h).show(ui, |ui| {
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
                            // Rückfrage mit Wartezeit statt „zweimal klicken“ (#276, `closing`).
                            if ui.button("🗑").on_hover_text(lang.t("Vorlage löschen …", "Delete template …")).clicked() {
                                actions.push(Action::TemplateDeleteAsk(i));
                            }
                            if ui.button(lang.t("Bearbeiten …", "Edit …")).clicked() {
                                actions.push(Action::TemplateEdit(i));
                            }
                            if !t.is_terminal() && ui.checkbox(&mut t.grid, lang.t("Raster", "Grid")).changed() {
                                changed = true;
                            }
                        });
                    });
                    ui.separator();
                }
            });
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
        let checking = w.checking(); // `git status` läuft im Hintergrund (#259)
        let name = self.entries.get(&w.id).map(|e| e.display_name()).unwrap_or_default();
        let mut keep = true;
        let mut action: Option<u8> = None; // 0 behalten, 1 entfernen, 2 mergen
        style::movable(ctx, Id::new("wt_close_dialog"), egui::Window::new(lang.t("Sitzung mit Worktree schließen", "Close session with worktree")))
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(lang.q(&name));
                ui.label(RichText::new(format!("{}: {}\n{}: {}", lang.t("Branch", "Branch"), wt.branch, lang.t("Ordner", "Folder"), wt.path)).small().color(style::dim(ui)));
                ui.add_space(S2);
                if checking {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new(lang.t("Prüfe auf nicht committete Änderungen …", "Checking for uncommitted changes …")).color(style::dim(ui)));
                    });
                } else if w.changes > 0 {
                    ui.colored_label(crate::style::ui().warning, format!("{} {}", w.changes, lang.t("Datei(en) mit nicht committeten Änderungen.", "file(s) with uncommitted changes.")));
                    ui.checkbox(&mut w.discard, lang.t("Änderungen verwerfen (Entfernen erzwingen)", "Discard changes (force removal)"));
                } else {
                    ui.label(RichText::new(lang.t("Keine offenen Änderungen.", "No pending changes.")).color(style::dim(ui)));
                }
                // Hintergrundprozesse im Job (#213): nennen und wahlweise beenden (sie können den Ordner belegt halten).
                if !w.lingering.is_empty() {
                    super::closing::lingering_section(ui, &w.lingering, lang, false);
                    ui.checkbox(&mut w.end_jobs, lang.t("Hintergrundprozesse beenden", "End background processes"));
                    let note = if w.end_jobs {
                        super::closing::end_all_warning(&w.lingering, lang)
                    } else {
                        lang.t("Laufende Hintergrundprozesse können den Ordner belegen – dann schlägt das Entfernen des Worktrees fehl.", "Running background processes can keep the folder busy – removing the worktree then fails.").to_string()
                    };
                    ui.label(RichText::new(note).small().color(style::dim(ui)));
                }
                ui.add_space(S2);
                ui.horizontal(|ui| {
                    if ui.button(lang.t("Behalten", "Keep")).on_hover_text(lang.t("Sitzung schließen, Worktree und Branch bleiben", "Close the session, keep worktree and branch")).clicked() {
                        action = Some(0);
                    }
                    if ui.add_enabled(!checking && (w.changes == 0 || w.discard), egui::Button::new(lang.t("Entfernen", "Remove"))).on_hover_text(lang.t("Worktree und Branch löschen", "Delete worktree and branch")).clicked() {
                        action = Some(1);
                    }
                    if ui.add_enabled(!checking && w.changes == 0, egui::Button::new(lang.t("Mergen & entfernen", "Merge & remove"))).on_hover_text(lang.t("Branch in den aktuellen Branch des Haupt-Repos mergen, dann aufräumen (bei Konflikt wird abgebrochen)", "Merge the branch into the main repo's current branch, then clean up (aborts on conflict)")).clicked() {
                        action = Some(2);
                    }
                    if ui.button(lang.t("Abbrechen", "Cancel")).clicked() {
                        keep = false;
                    }
                });
            });
        if let Some(a) = action {
            let id = w.id;
            if w.end_jobs {
                self.end_jobs(&super::closing::job_refs(&w.lingering));
            }
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
        // Strg+Enter gehört dem Composer nur, wenn kein Terminal den Fokus hat (sonst bekommt es das Terminal, #255).
        let ctrl_enter = !self.terminal_has_focus(ctx) && ctx.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::Enter));
        let mut send = ctrl_enter;
        let (mut paste_only, mut to_queue, mut to_all, mut save_prompt) = (false, false, false, false);
        style::movable(ctx, Id::new("composer_win"), egui::Window::new(lang.t("Prompt-Composer", "Prompt composer"))).open(&mut open).default_size([680.0, 420.0]).show(ctx, |ui| {
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
                if ui.add_enabled(!self.composer_text.trim().is_empty(), egui::Button::new(RichText::new(lang.t("Senden (Strg+Enter)", "Send (Ctrl+Enter)")).strong()).fill(crate::style::ui().chat_send)).clicked() {
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
        style::movable(ctx, Id::new("prompt_lib_win"), egui::Window::new(lang.t("Prompt-Bibliothek", "Prompt library")))
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
        style::movable(ctx, Id::new("notes_win"), egui::Window::new(format!("{} – {}", lang.t("Notizen", "Notes"), e.display_name())))
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

/// Zeilen eines zweispaltigen Grids für die Claude-Startoptionen (#161): Modell, Effort, Permission-Mode, Agent
/// (Reiter *Claude*, #292). Leere Auswahl = Standard von Claude Code (Flag wird nicht gesetzt). `false`, solange ein
/// Name ungültig ist. `bypassPermissions` nur im Dialog „Neue Sitzung“ und in den Einstellungen der Sitzung (gilt nur
/// für diese Sitzung) und mit Warnung; in den Standards und in Vorlagen gibt es es nicht.
pub(super) fn claude_basic_rows(ui: &mut Ui, lang: Lang, o: &mut ClaudeOpts, salt: &str, place: OptsPlace) -> bool {
    // Nie in den Standards und nie in einer Vorlage (die Datei kann weitergegeben werden).
    let allow_bypass = !matches!(place, OptsPlace::Defaults | OptsPlace::Template);
    use crate::claude_opts::{valid_name, BYPASS, EFFORTS, MAX_NAME, MODEL_ALIASES, PERMISSION_MODES};
    let default = lang.t("Standard von Claude Code", "Claude Code default");
    let shown = |v: &str| match v {
        "" => default.to_string(),
        "default" => lang.t("default (bei allem fragen)", "default (always ask)").to_string(),
        _ => v.to_string(),
    };
    let bad_name = |v: &str| !v.trim().is_empty() && !valid_name(v.trim());
    let hint = |ui: &mut Ui, text: &str, warn: bool| {
        ui.label("");
        let c = if warn { crate::style::ui().error } else { style::dim(ui) };
        hint_label(ui, text, c);
        ui.end_row();
    };
    let name_err = lang.t("Nur Buchstaben, Ziffern und . _ : - (höchstens 100 Zeichen).", "Only letters, digits and . _ : - (at most 100 characters).");
    debug_assert_eq!(MAX_NAME, 100);

    ui.label(lang.t("Modell", "Model"));
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut o.model).hint_text(default).char_limit(MAX_NAME).desired_width(220.0));
        ui.menu_button("▾", |ui| {
            for m in std::iter::once("").chain(MODEL_ALIASES) {
                if ui.button(shown(m)).clicked() {
                    o.model = m.to_string();
                    ui.close();
                }
            }
        });
    });
    ui.end_row();
    if bad_name(&o.model) {
        hint(ui, name_err, true);
    }

    ui.label("Effort");
    egui::ComboBox::from_id_salt(format!("{salt}_effort")).selected_text(shown(&o.effort)).show_ui(ui, |ui| {
        for e in std::iter::once("").chain(EFFORTS) {
            ui.selectable_value(&mut o.effort, e.to_string(), shown(e));
        }
    });
    ui.end_row();

    ui.label("Permission-Mode");
    egui::ComboBox::from_id_salt(format!("{salt}_perm")).selected_text(shown(&o.permission_mode)).show_ui(ui, |ui| {
        let bypass: &[&str] = if allow_bypass { &[BYPASS] } else { &[] };
        for m in std::iter::once("").chain(PERMISSION_MODES).chain(bypass.iter().copied()) {
            ui.selectable_value(&mut o.permission_mode, m.to_string(), shown(m));
        }
    });
    ui.end_row();
    if !allow_bypass && o.permission_mode == BYPASS {
        o.permission_mode.clear();
    }
    if o.permission_mode == BYPASS {
        hint(
            ui,
            lang.t(
                "⚠ bypassPermissions: Claude fragt bei nichts mehr nach (Dateien, Befehle, Netz). Nur in einer abgeschotteten Umgebung verwenden. Gilt nur für diese Sitzung und wird nicht als Vorauswahl gemerkt; Agenten über MCP können diesen Modus nie setzen.",
                "⚠ bypassPermissions: Claude no longer asks before anything (files, commands, network). Use only in an isolated environment. Applies only to this session and is not remembered as a preselection; agents via MCP can never set this mode.",
            ),
            true,
        );
    }

    ui.label("Agent");
    ui.add(egui::TextEdit::singleline(&mut o.agent).hint_text(lang.t("Name eines Claude-Agenten (optional)", "Name of a Claude agent (optional)")).char_limit(MAX_NAME).desired_width(220.0));
    ui.end_row();
    if bad_name(&o.agent) {
        hint(ui, name_err, true);
    }
    o.model = o.model.trim().to_string();
    o.agent = o.agent.trim().to_string();
    !bad_name(&o.model) && !bad_name(&o.agent)
}

/// Kurzhilfe zu einem Schalter (Tooltip).
fn switch_help(name: &str, lang: Lang) -> &'static str {
    match name {
        "chrome" => lang.t("Claude-in-Chrome-Integration einschalten.", "Turn on the Claude in Chrome integration."),
        "no-chrome" => lang.t("Claude-in-Chrome-Integration ausschalten.", "Turn off the Claude in Chrome integration."),
        "ide" => lang.t("Beim Start automatisch mit einer IDE verbinden.", "Connect to an IDE automatically at start."),
        "verbose" => lang.t("Ausführliche Ausgabe.", "Verbose output."),
        "disable-slash-commands" => lang.t("Alle Slash-Befehle abschalten.", "Disable all slash commands."),
        "safe-mode" => lang.t("Hooks und MCP-Server abschalten – Paneldeck erkennt dann Status und Freigabe-Fragen nicht. Nur für dich, nie für Agenten.", "Turn off hooks and MCP servers – Paneldeck then can't detect status and permission prompts. Only for you, never for agents."),
        _ => lang.t("Wird unverändert an Claude Code übergeben (siehe claude --help).", "Passed to Claude Code as is (see claude --help)."),
    }
}

/// Wo die Claude-Optionen bearbeitet werden: bestimmt `bypassPermissions` (nicht in den Standards) und die Hinweise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OptsPlace {
    /// Dialog „Neue Sitzung“ (Sitzung des Nutzers).
    NewSession,
    /// Einstellungen einer bestehenden Sitzung (auch einer, die ein Agent angelegt hat).
    Session,
    /// Standards unter Einstellungen → Claude.
    Defaults,
    /// Editor einer Terminal-Vorlage (#244): wie die Standards ohne `bypassPermissions`; die Datei kann weitergegeben
    /// werden, vor dem Start erweiterter Optionen wird nachgefragt.
    Template,
}

/// Ergebnis einer Pfadprüfung, je Pfad zwischengespeichert (keine Dateisystem-Zugriffe in jedem Bild).
type PathCache = std::collections::HashMap<(String, bool), (Result<(), crate::claude_opts::PathIssue>, f64)>;

/// Wie lange ein Prüfergebnis gilt (Sekunden).
const PATH_CACHE_SECS: f64 = 3.0;

/// Prüfung aller Claude-Optionen eines Dialogs – auch der Reiter, die gerade nicht zu sehen sind (#292): ein ungültiger
/// Wert sperrt „Erstellen“/„Speichern“ und gibt dem Reiter einen roten Punkt.
#[derive(Default, Clone, Debug)]
pub(super) struct OptsCheck {
    entry_errs: std::collections::HashMap<(String, bool), String>,
    dir_err: Option<String>,
    plugin_err: Option<String>,
    mcp_err: Option<String>,
    allowed_bad: bool,
    disallowed_bad: bool,
    text_bad: bool,
    sys_bad: bool,
    fallback_bad: bool,
    budget_bad: bool,
    compact_bad: bool,
    name_bad: bool,
    /// Linux-Pfade (WSL) in einer Sitzung, die nicht in WSL läuft: werden beim Start weggelassen (Warnung, kein Fehler –
    /// sie können aus den Standards oder der letzten Wahl stammen).
    linux_ignored: Vec<String>,
}

impl OptsCheck {
    pub(super) fn basic_ok(&self) -> bool {
        !self.name_bad
    }

    pub(super) fn access_ok(&self) -> bool {
        self.dir_err.is_none() && self.plugin_err.is_none() && self.mcp_err.is_none() && !self.allowed_bad && !self.disallowed_bad
    }

    pub(super) fn advanced_ok(&self) -> bool {
        !self.text_bad && !self.sys_bad && !self.fallback_bad && !self.budget_bad && !self.compact_bad
    }

    pub(super) fn ok(&self) -> bool {
        self.basic_ok() && self.access_ok() && self.advanced_ok()
    }
}

/// Alle Werte prüfen: Form immer, Pfade im Dateisystem höchstens alle [`PATH_CACHE_SECS`] je Pfad (Zwischenspeicher im
/// Speicher von egui, je `salt`). Linux-Pfade (WSL) sind in einer Sitzung, die nicht in WSL läuft, ungültig; als
/// Standard und in Vorlagen erlaubt (gelten nur in WSL-Sitzungen).
pub(super) fn check_opts(ctx: &egui::Context, lang: Lang, o: &ClaudeOpts, salt: &str, place: OptsPlace, target: &folder_picker::Target) -> OptsCheck {
    use crate::claude_opts::{check_path, valid_autocompact, valid_budget, valid_name, valid_path, valid_system_prompt, valid_text, valid_tools, wsl_linux_path, PathIssue, MAX_DIRS};
    let cache_id = Id::new(("claude_more_paths", salt));
    let now = ctx.input(|i| i.time);
    let mut cache: PathCache = ctx.data(|d| d.get_temp(cache_id)).unwrap_or_default();
    let mut check = |p: &str, dir: bool| -> Result<(), PathIssue> {
        if wsl_linux_path(p) {
            // Linux-Pfad: vom Windows-Programm aus ungeprüft (siehe `wsl_linux_path`).
            return Ok(());
        }
        if !valid_path(p) {
            return Err(PathIssue::Invalid);
        }
        let key = (p.to_string(), dir);
        match cache.get(&key) {
            Some((r, t)) if now - t < PATH_CACHE_SECS => *r,
            _ => {
                let r = check_path(p, dir);
                cache.insert(key, (r, now));
                r
            }
        }
    };
    let path_err = |r: Result<(), PathIssue>, p: &str, dir: bool| -> Option<String> {
        let why = match r {
            Ok(()) => return None,
            Err(PathIssue::Invalid) => lang.t("kein absoluter lokaler Pfad (oder Zeichen \" % ! ^ & | < >)", "not an absolute local path (or contains \" % ! ^ & | < >)"),
            Err(PathIssue::Network) => lang.t("Netzwerkpfade sind nicht erlaubt", "network paths are not allowed"),
            Err(PathIssue::Missing) if dir => lang.t("Ordner existiert nicht", "folder does not exist"),
            Err(PathIssue::Missing) => lang.t("Datei existiert nicht", "file does not exist"),
        };
        Some(format!("{}: {why}", lang.q(p)))
    };
    let wsl_session = matches!(target, folder_picker::Target::Wsl { .. });
    let linux_bad = |p: &str| wsl_linux_path(p) && !wsl_session && matches!(place, OptsPlace::NewSession | OptsPlace::Session);
    let e = &o.extra;
    let mut c = OptsCheck::default();
    let mut ignored: Vec<String> = Vec::new();
    let mut list_err = |entry_errs: &mut std::collections::HashMap<(String, bool), String>, list: &[String], dir: bool, too_many: &str| -> Option<String> {
        let items: Vec<&str> = list.iter().map(|d| d.trim()).filter(|d| !d.is_empty()).collect();
        let mut first = (items.len() > MAX_DIRS).then(|| format!("{too_many} {MAX_DIRS}"));
        for d in items {
            if linux_bad(d) {
                ignored.push(d.to_string());
                continue;
            }
            if let Some(err) = path_err(check(d, dir), d, dir) {
                entry_errs.insert((d.to_string(), dir), err.clone());
                first.get_or_insert(err);
            }
        }
        first
    };
    let folders = lang.t("Höchstens so viele Ordner:", "At most this many folders:");
    c.dir_err = list_err(&mut c.entry_errs, &e.add_dirs, true, folders);
    c.plugin_err = list_err(&mut c.entry_errs, &e.plugin_dirs, true, folders);
    c.mcp_err = list_err(&mut c.entry_errs, &e.mcp_configs, false, lang.t("Höchstens so viele Dateien:", "At most this many files:"));
    c.linux_ignored = ignored;
    ctx.data_mut(|d| d.insert_temp(cache_id, cache));
    let filled = |s: &str| !s.trim().is_empty();
    let tools_bad = |s: &str| filled(s) && !valid_tools(s.trim());
    let bad_name = |v: &str| filled(v) && !valid_name(v.trim());
    c.allowed_bad = tools_bad(&e.allowed_tools);
    c.disallowed_bad = tools_bad(&e.disallowed_tools);
    c.text_bad = filled(&e.append_system_prompt) && !valid_text(e.append_system_prompt.trim());
    c.sys_bad = filled(&e.system_prompt) && !valid_system_prompt(&crate::claude_opts::normalize_multiline(&e.system_prompt));
    c.fallback_bad = bad_name(&e.fallback_model);
    c.budget_bad = filled(&e.max_budget_usd) && !valid_budget(e.max_budget_usd.trim());
    c.compact_bad = filled(&e.autocompact) && !valid_autocompact(e.autocompact.trim());
    c.name_bad = bad_name(&o.model) || bad_name(&o.agent);
    c
}

/// Hinweiszeile im zweispaltigen Raster (links leer).
fn grid_hint(ui: &mut Ui, text: &str, c: Color32) {
    ui.label("");
    let r = hint_label(ui, text, c);
    #[cfg(test)]
    ui.data_mut(|d| {
        let n = d.get_temp::<usize>(Id::new("dlg_hint_n")).unwrap_or(0);
        d.insert_temp(Id::new(("dlg_mark", "more_hint", n)), r.rect);
        d.insert_temp(Id::new("dlg_hint_n"), n + 1);
    });
    let _ = r;
    ui.end_row();
}

#[allow(non_snake_case)]
fn RED() -> Color32 {
    crate::style::ui().error
}
#[allow(non_snake_case)]
fn AMBER() -> Color32 {
    crate::style::ui().warning
}

/// Reiter *Zugriff* (#292): zusätzliche Ordner, Plugin-Ordner, MCP-Konfigurationen (mit `--strict-mcp-config`) als
/// Listen mit + und − (#289), erlaubte und verbotene Werkzeuge; Warnung bei Optionen, die Zugriffe erweitern.
pub(super) fn claude_access_rows(ui: &mut Ui, lang: Lang, o: &mut ClaudeOpts, salt: &str, place: OptsPlace, target: &folder_picker::Target, chk: &OptsCheck) {
    use super::list_edit::{list_editor, pick_entry, ListKind};
    use crate::claude_opts::{MAX_DIRS, MAX_TOOLS_LEN};
    let e = &mut o.extra;
    // Standards und Vorlagen gelten für jede Shell: dort auch Ordner aus WSL (als Linux-Pfad, nur für WSL-Sitzungen).
    let linux_ok = matches!(place, OptsPlace::Defaults | OptsPlace::Template);
    let dir_issue = |p: &str| chk.entry_errs.get(&(p.trim().to_string(), true)).cloned();
    let file_issue = |p: &str| chk.entry_errs.get(&(p.trim().to_string(), false)).cloned();
    ui.label(lang.t("Zusätzliche Ordner", "Extra folders")).on_hover_text("--add-dir");
    list_editor(ui, lang, &format!("{salt}_add_dir"), &mut e.add_dirs, ListKind::Folders, MAX_DIRS, &dir_issue, &mut || pick_entry(ListKind::Folders, target, lang, linux_ok));
    ui.end_row();
    if let Some(err) = &chk.dir_err {
        grid_hint(ui, err, RED());
    }

    ui.label(lang.t("Plugin-Ordner", "Plugin folders")).on_hover_text("--plugin-dir");
    list_editor(ui, lang, &format!("{salt}_plugin_dir"), &mut e.plugin_dirs, ListKind::Folders, MAX_DIRS, &dir_issue, &mut || pick_entry(ListKind::Folders, target, lang, linux_ok));
    ui.end_row();
    if let Some(err) = &chk.plugin_err {
        grid_hint(ui, err, RED());
    }

    ui.label(lang.t("MCP-Konfiguration", "MCP config")).on_hover_text("--mcp-config");
    list_editor(ui, lang, &format!("{salt}_mcp_config"), &mut e.mcp_configs, ListKind::JsonFiles, MAX_DIRS, &file_issue, &mut || pick_entry(ListKind::JsonFiles, target, lang, linux_ok));
    ui.end_row();
    if let Some(err) = &chk.mcp_err {
        grid_hint(ui, err, RED());
    }
    // Nur mit mindestens einer MCP-Konfiguration wählbar (ohne sie wäre es wirkungslos).
    ui.label("");
    let has_mcp = !e.mcp_configs.is_empty();
    if !has_mcp {
        e.strict_mcp_config = false;
    }
    let strict = ui
        .add_enabled(has_mcp, egui::Checkbox::new(&mut e.strict_mcp_config, lang.t("Nur MCP-Server aus diesen Dateien", "Only MCP servers from these files")))
        .on_hover_text(lang.t(
            "--strict-mcp-config: alle anderen MCP-Konfigurationen (Nutzer, Projekt, auch Paneldecks Agenten-Schnittstelle) werden ignoriert.",
            "--strict-mcp-config: all other MCP configurations (user, project, also Paneldeck's agent interface) are ignored.",
        ))
        .on_disabled_hover_text(lang.t("Erst eine MCP-Konfiguration hinzufügen.", "Add an MCP config first."));
    dlg_mark(ui, "strict_mcp", &strict);
    ui.end_row();

    let tools_err = lang.t("Nur Buchstaben, Ziffern, Leerzeichen und _ . , : * ( ) / @ - ~ = + [ ] ? # \\ (nicht am Anfang -, nicht am Ende \\).", "Only letters, digits, spaces and _ . , : * ( ) / @ - ~ = + [ ] ? # \\ (no leading -, no trailing \\).");
    ui.label(lang.t("Erlaubte Werkzeuge", "Allowed tools")).on_hover_text("--allowed-tools");
    ui.add(egui::TextEdit::singleline(&mut e.allowed_tools).hint_text("Bash(git log:*) Read").char_limit(MAX_TOOLS_LEN).desired_width(FIELD_W));
    ui.end_row();
    if chk.allowed_bad {
        grid_hint(ui, tools_err, RED());
    }
    ui.label(lang.t("Verbotene Werkzeuge", "Disallowed tools")).on_hover_text("--disallowed-tools");
    ui.add(egui::TextEdit::singleline(&mut e.disallowed_tools).hint_text("WebFetch Bash(curl:*)").char_limit(MAX_TOOLS_LEN).desired_width(FIELD_W));
    ui.end_row();
    if chk.disallowed_bad {
        grid_hint(ui, tools_err, RED());
    }

    if !chk.linux_ignored.is_empty() {
        let list = chk.linux_ignored.iter().map(|p| lang.q(p)).collect::<Vec<_>>().join(", ");
        grid_hint(
            ui,
            &format!("⚠ {}: {list}", lang.t("Linux-Pfade gelten nur in WSL-Sitzungen – für diese Shell werden sie weggelassen", "Linux paths apply only to WSL sessions – ignored for this shell")),
            AMBER(),
        );
    }
    if e.has_risky() {
        grid_hint(
            ui,
            match place {
                OptsPlace::Defaults => lang.t(
                    "⚠ Zusätzliche Ordner, Plugin-Ordner, erlaubte Werkzeuge und MCP-Konfigurationen erweitern, worauf Claude zugreifen und was es ohne Rückfrage tun darf. Als Standard gelten sie nur für Sitzungen, die du selbst startest – nie für Sitzungen von Agenten.",
                    "⚠ Extra folders, plugin folders, allowed tools and MCP configs widen what Claude may access and do without asking. As defaults they apply only to sessions you start yourself – never to sessions created by agents.",
                ),
                OptsPlace::NewSession => lang.t(
                    "⚠ Zusätzliche Ordner, Plugin-Ordner, erlaubte Werkzeuge und MCP-Konfigurationen erweitern, worauf Claude in dieser Sitzung zugreifen und was es ohne Rückfrage tun darf.",
                    "⚠ Extra folders, plugin folders, allowed tools and MCP configs widen what Claude may access and do without asking in this session.",
                ),
                OptsPlace::Template => lang.t(
                    "⚠ Zusätzliche Ordner, Plugin-Ordner, erlaubte Werkzeuge und MCP-Konfigurationen erweitern, worauf Claude zugreifen und was es ohne Rückfrage tun darf. Sie werden mit der Vorlage gespeichert (templates.json lässt sich weitergeben): Vor dem Start zeigt Paneldeck die Werte und fragt nach; Sitzungen, die ein Agent aus der Vorlage startet, bekommen sie nie.",
                    "⚠ Extra folders, plugin folders, allowed tools and MCP configs widen what Claude may access and do without asking. They are saved with the template (templates.json can be shared): before it starts, Paneldeck shows the values and asks; sessions an agent starts from the template never get them.",
                ),
                OptsPlace::Session => lang.t(
                    "⚠ Zusätzliche Ordner, Plugin-Ordner, erlaubte Werkzeuge und MCP-Konfigurationen erweitern, worauf Claude in dieser Sitzung zugreifen und was es ohne Rückfrage tun darf – ab dem nächsten Start, auch wenn ein Agent die Sitzung angelegt hat.",
                    "⚠ Extra folders, plugin folders, allowed tools and MCP configs widen what Claude may access and do without asking in this session – from its next start, even if an agent created the session.",
                ),
            },
            AMBER(),
        );
    }
    let dim = style::dim(ui);
    grid_hint(
        ui,
        lang.t(
            "Nur du setzt diese Werte; Agenten bekommen sie nie. Ordner und Dateien werden bei jedem Start geprüft, fehlende mit Hinweis weggelassen. Linux-Pfade (aus WSL) gelten nur in WSL-Sitzungen.",
            "Only you set these values; agents never get them. Folders and files are checked at every start; missing ones are left out with a notice. Linux paths (from WSL) apply only to WSL sessions.",
        ),
        dim,
    );
}

/// Reiter *Erweitert* (#292): Schalter (auch `--safe-mode`, nur für den Nutzer), eigener und ergänzter System-Prompt,
/// Ersatzmodell, Budget und Auto-Komprimierung.
pub(super) fn claude_advanced_rows(ui: &mut Ui, lang: Lang, o: &mut ClaudeOpts, salt: &str, place: OptsPlace, chk: &OptsCheck) {
    use crate::claude_opts::{agent_switch, MAX_NAME, MAX_SYSTEM_PROMPT_LEN, MAX_TEXT_LEN, SWITCHES, USER_SWITCHES};
    let e = &mut o.extra;
    ui.label(lang.t("Schalter", "Switches"));
    // Feste Breite (#241): die Zeile bricht immer gleich um, das Raster springt nicht.
    let sw = ui.vertical(|ui| {
        ui.set_max_width(HINT_W);
        ui.horizontal_wrapped(|ui| {
            ui.set_max_width(HINT_W);
            for (n, f) in SWITCHES.iter().chain(USER_SWITCHES.iter()) {
                let mut on = e.flags.iter().any(|x| x == n);
                if ui.checkbox(&mut on, *f).on_hover_text(switch_help(n, lang)).changed() {
                    e.flags.retain(|x| x != n);
                    if on {
                        e.flags.push(n.to_string());
                        // chrome und no-chrome schließen sich aus.
                        let other = match *n {
                            "chrome" => "no-chrome",
                            "no-chrome" => "chrome",
                            _ => "",
                        };
                        e.flags.retain(|x| x != other);
                    }
                    e.flags = SWITCHES.iter().chain(USER_SWITCHES.iter()).filter(|(s, _)| e.flags.iter().any(|x| x == s)).map(|(s, _)| s.to_string()).collect();
                }
            }
        });
    });
    #[cfg(test)]
    ui.data_mut(|d| d.insert_temp(Id::new(("dlg_mark", "switches", 0usize)), sw.response.rect));
    let _ = sw;
    ui.end_row();
    if e.flags.iter().any(|f| !agent_switch(f)) {
        grid_hint(
            ui,
            match place {
                OptsPlace::Template => lang.t(
                    "⚠ --safe-mode schaltet Hooks und MCP-Server ab: Paneldeck erkennt dann weder Status noch Freigabe-Fragen der Sitzung – auch der Schutz, dass Agenten keine Freigaben beantworten, greift dort nicht. Vor dem Start dieser Vorlage fragt Paneldeck nach; startet ein Agent sie, gilt es nie.",
                    "⚠ --safe-mode turns off hooks and MCP servers: Paneldeck then sees neither the status nor the permission prompts of the session – the protection that keeps agents from answering permission prompts does not work there either. Paneldeck asks before this template starts; when an agent starts it, it never applies.",
                ),
                OptsPlace::Defaults => lang.t(
                    "⚠ --safe-mode schaltet Hooks und MCP-Server ab: Paneldeck erkennt dann weder Status noch Freigabe-Fragen dieser Sitzungen – auch der Schutz, dass Agenten keine Freigaben beantworten, greift dort nicht. Gilt nur für Sitzungen, die du selbst startest, nie für Sitzungen von Agenten.",
                    "⚠ --safe-mode turns off hooks and MCP servers: Paneldeck then sees neither the status nor the permission prompts of these sessions – the protection that keeps agents from answering permission prompts does not work there either. Applies only to sessions you start yourself, never to sessions created by agents.",
                ),
                _ => lang.t(
                    "⚠ --safe-mode schaltet Hooks und MCP-Server ab: Paneldeck erkennt dann weder Status noch Freigabe-Fragen dieser Sitzung – auch der Schutz, dass Agenten keine Freigaben beantworten, greift hier nicht.",
                    "⚠ --safe-mode turns off hooks and MCP servers: Paneldeck then sees neither the status nor the permission prompts of this session – the protection that keeps agents from answering permission prompts does not work here either.",
                ),
            },
            AMBER(),
        );
    }

    ui.label(lang.t("Eigener System-Prompt", "System prompt")).on_hover_text("--system-prompt");
    let sp = ui.add(
        egui::TextEdit::multiline(&mut e.system_prompt)
            .hint_text(lang.t("Ersetzt den System-Prompt von Claude Code (optional)", "Replaces Claude Code's system prompt (optional)"))
            .char_limit(MAX_SYSTEM_PROMPT_LEN)
            .desired_rows(3)
            .desired_width(FIELD_W),
    );
    super::opts_tabs::multiline(ui, salt, &sp);
    dlg_mark(ui, "system_prompt", &sp);
    ui.end_row();
    if chk.sys_bad {
        grid_hint(ui, lang.t("Ohne \" % ! ^ & | < > \\ und Steuerzeichen außer Zeilenumbrüchen, nicht mit - beginnend.", "Without \" % ! ^ & | < > \\ and control characters other than line breaks, not starting with -."), RED());
    }
    if !e.system_prompt.trim().is_empty() {
        grid_hint(
            ui,
            match place {
                OptsPlace::Template => lang.t(
                    "⚠ Ersetzt den eingebauten System-Prompt von Claude Code ganz (Anweisungen zu Werkzeugen, Sicherheit und Arbeitsweise) – Claude verhält sich dann womöglich ganz anders. „System-Prompt ergänzen“ wird danach angehängt. Vor dem Start dieser Vorlage zeigt Paneldeck den Text und fragt nach; Sitzungen von Agenten bekommen ihn nie. In PowerShell und cmd werden Zeilenumbrüche zu Leerzeichen.",
                    "⚠ Replaces Claude Code's built-in system prompt completely (instructions on tools, safety and how to work) – Claude may then behave very differently. “Append system prompt” is added after it. Paneldeck shows the text and asks before this template starts; sessions created by agents never get it. In PowerShell and cmd, line breaks become spaces.",
                ),
                _ => lang.t(
                    "⚠ Ersetzt den eingebauten System-Prompt von Claude Code ganz (Anweisungen zu Werkzeugen, Sicherheit und Arbeitsweise) – Claude verhält sich dann womöglich ganz anders. „System-Prompt ergänzen“ wird danach angehängt. Gilt nie für Sitzungen von Agenten. In PowerShell und cmd werden Zeilenumbrüche zu Leerzeichen.",
                    "⚠ Replaces Claude Code's built-in system prompt completely (instructions on tools, safety and how to work) – Claude may then behave very differently. “Append system prompt” is added after it. Never applies to sessions created by agents. In PowerShell and cmd, line breaks become spaces.",
                ),
            },
            AMBER(),
        );
    }

    ui.label(lang.t("System-Prompt ergänzen", "Append system prompt")).on_hover_text("--append-system-prompt");
    ui.add(egui::TextEdit::singleline(&mut e.append_system_prompt).hint_text(lang.t("Text (eine Zeile, optional)", "Text (one line, optional)")).char_limit(MAX_TEXT_LEN).desired_width(FIELD_W));
    ui.end_row();
    if chk.text_bad {
        grid_hint(ui, lang.t("Eine Zeile ohne \" % ! ^ & | < > \\, nicht mit - beginnend.", "One line without \" % ! ^ & | < > \\, not starting with -."), RED());
    }

    ui.label(lang.t("Ersatzmodell", "Fallback model")).on_hover_text("--fallback-model");
    ui.add(egui::TextEdit::singleline(&mut e.fallback_model).hint_text(lang.t("z. B. sonnet (optional)", "e.g. sonnet (optional)")).char_limit(MAX_NAME).desired_width(160.0));
    ui.end_row();
    if chk.fallback_bad {
        grid_hint(ui, lang.t("Nur Buchstaben, Ziffern und . _ : - (höchstens 100 Zeichen).", "Only letters, digits and . _ : - (at most 100 characters)."), RED());
    }

    ui.label(lang.t("Budget (USD)", "Max budget (USD)")).on_hover_text("--max-budget-usd");
    ui.add(egui::TextEdit::singleline(&mut e.max_budget_usd).hint_text(lang.t("z. B. 5", "e.g. 5")).char_limit(12).desired_width(80.0));
    ui.end_row();
    if chk.budget_bad {
        grid_hint(ui, lang.t("Eine positive Zahl, z. B. 5 oder 2.50.", "A positive number, e.g. 5 or 2.50."), RED());
    }

    ui.label(lang.t("Auto-Komprimierung", "Auto-compact")).on_hover_text("--autocompact");
    ui.add(egui::TextEdit::singleline(&mut e.autocompact).hint_text(lang.t("auto oder Tokens, z. B. 200k", "auto or tokens, e.g. 200k")).char_limit(12).desired_width(120.0));
    ui.end_row();
    if chk.compact_bad {
        grid_hint(ui, lang.t("„auto“ oder eine Tokenzahl, z. B. 200000 oder 200k.", "“auto” or a number of tokens, e.g. 200000 or 200k."), RED());
    }
}

/// Breite der Eingabefelder in den Reitern.
pub(super) const FIELD_W: f32 = 400.0;

/// Gespeicherte Claude-Standards des Nutzers (setzt die App in jedem Bild, für Zählen und Zurücksetzen in den Reitern).
pub(super) fn user_defaults(ctx: &egui::Context) -> ClaudeOpts {
    ctx.data(|d| d.get_temp::<ClaudeOpts>(Id::new("claude_user_defaults"))).unwrap_or_default()
}

/// Zahl der Werte je Reiter (*Claude*, *Zugriff*, *Erweitert*), die von `d` abweichen.
pub(super) fn tab_counts(o: &ClaudeOpts, d: &ClaudeOpts) -> [usize; 3] {
    let (o, d) = (o.trimmed(), d.trimmed());
    let n = |v: &[bool]| v.iter().filter(|x| **x).count();
    let (a, b) = (&o.extra, &d.extra);
    let mut fa = a.flags.clone();
    let mut fb = b.flags.clone();
    fa.sort();
    fb.sort();
    [
        n(&[o.model != d.model, o.effort != d.effort, o.permission_mode != d.permission_mode, o.agent != d.agent]),
        n(&[a.add_dirs != b.add_dirs, a.plugin_dirs != b.plugin_dirs, a.mcp_configs != b.mcp_configs, a.strict_mcp_config != b.strict_mcp_config, a.allowed_tools != b.allowed_tools, a.disallowed_tools != b.disallowed_tools]),
        n(&[fa != fb, a.system_prompt != b.system_prompt, a.append_system_prompt != b.append_system_prompt, a.fallback_model != b.fallback_model, a.max_budget_usd != b.max_budget_usd, a.autocompact != b.autocompact]),
    ]
}

/// Werte eines Reiters auf `d` zurücksetzen.
pub(super) fn reset_tab(o: &mut ClaudeOpts, d: &ClaudeOpts, tab: super::opts_tabs::Tab) {
    use super::opts_tabs::Tab;
    let (a, b) = (&mut o.extra, &d.extra);
    match tab {
        Tab::Claude => {
            o.model = d.model.clone();
            o.effort = d.effort.clone();
            o.permission_mode = d.permission_mode.clone();
            o.agent = d.agent.clone();
        }
        Tab::Access => {
            a.add_dirs = b.add_dirs.clone();
            a.plugin_dirs = b.plugin_dirs.clone();
            a.mcp_configs = b.mcp_configs.clone();
            a.strict_mcp_config = b.strict_mcp_config;
            a.allowed_tools = b.allowed_tools.clone();
            a.disallowed_tools = b.disallowed_tools.clone();
        }
        Tab::Advanced => {
            a.flags = b.flags.clone();
            a.system_prompt = b.system_prompt.clone();
            a.append_system_prompt = b.append_system_prompt.clone();
            a.fallback_model = b.fallback_model.clone();
            a.max_budget_usd = b.max_budget_usd.clone();
            a.autocompact = b.autocompact.clone();
        }
        Tab::General => {}
    }
}

/// Beschriftung der drei Claude-Reiter: Zahl der Abweichungen von `d`, roter Punkt bei ungültigen Werten, orange bei
/// Warnungen (bypassPermissions, erweiterte Zugriffe, --safe-mode, eigener System-Prompt).
pub(super) fn claude_tab_infos(o: &ClaudeOpts, d: &ClaudeOpts, chk: &OptsCheck) -> [super::opts_tabs::TabInfo; 3] {
    use super::opts_tabs::{Mark, Tab, TabInfo};
    let [c1, c2, c3] = tab_counts(o, d);
    let t = o.trimmed();
    let mark = |ok: bool, warn: bool| if !ok { Mark::Error } else if warn { Mark::Warn } else { Mark::None };
    [
        TabInfo { tab: Tab::Claude, count: c1, mark: mark(chk.basic_ok(), t.permission_mode == crate::claude_opts::BYPASS) },
        TabInfo { tab: Tab::Access, count: c2, mark: mark(chk.access_ok(), t.extra.has_risky() || !chk.linux_ignored.is_empty()) },
        TabInfo { tab: Tab::Advanced, count: c3, mark: mark(chk.advanced_ok(), !t.extra.system_prompt.is_empty() || t.extra.flags.iter().any(|f| !crate::claude_opts::agent_switch(f))) },
    ]
}

/// Inhalt eines Claude-Reiters (*Claude*, *Zugriff*, *Erweitert*) als zweispaltiges Raster, darunter „Auf Standards
/// zurücksetzen“. `before`/`after`: weitere Zeilen des Reiters *Claude* (z. B. Konversation, Start-Prompt).
#[allow(clippy::too_many_arguments)]
pub(super) fn claude_tab(
    ui: &mut Ui,
    lang: Lang,
    o: &mut ClaudeOpts,
    salt: &str,
    place: OptsPlace,
    target: &folder_picker::Target,
    d: &ClaudeOpts,
    chk: &OptsCheck,
    tab: super::opts_tabs::Tab,
    before: impl FnOnce(&mut Ui),
    after: impl FnOnce(&mut Ui),
) {
    use super::opts_tabs::Tab;
    egui::Grid::new((salt, "opts_tab_grid", tab)).num_columns(2).spacing([8.0, 8.0]).min_col_width(140.0).show(ui, |ui| match tab {
        Tab::Claude => {
            before(ui);
            claude_basic_rows(ui, lang, o, salt, place);
            after(ui);
        }
        Tab::Access => claude_access_rows(ui, lang, o, salt, place, target, chk),
        Tab::Advanced => claude_advanced_rows(ui, lang, o, salt, place, chk),
        Tab::General => {}
    });
    ui.add_space(6.0);
    let i = match tab {
        Tab::Claude => 0,
        Tab::Access => 1,
        Tab::Advanced => 2,
        Tab::General => return,
    };
    if super::opts_tabs::reset_button(ui, lang, tab_counts(o, d)[i] > 0) {
        reset_tab(o, d, tab);
    }
}

/// Kommandozeile für die Vorschau: `claude` und die Optionen, gequotet für die Shell (ohne IDs und Hooks), mit den
/// freien Resten alter `claude_args`. Ohne Zugriff aufs Dateisystem ([`ClaudeOpts::preview`]).
pub(super) fn preview_line(o: &ClaudeOpts, shell: Option<&ShellSpec>, legacy_rest: &str) -> String {
    use crate::claude_opts::PathStyle;
    let kind = shell.map(crate::shells::quote_kind).unwrap_or(crate::quote::Kind::Pwsh);
    let style = shell.map(|s| PathStyle::for_shell(&s.id, &s.program, kind)).unwrap_or(PathStyle::Native);
    format!("{}{}", style.claude_program(), o.preview(kind, style, legacy_rest))
}

/// Shell und freie Reste für die Vorschau eines Dialogs (`salt`), so aufgelöst wie beim Start (setzt die App:
/// Profil mit eigenem Programm, Standard-Shell).
pub(super) fn preview_target_id(salt: &str) -> Id {
    Id::new(("claude_preview_shell", salt.to_string()))
}

/// Zuschlag für das, was Paneldeck beim Start noch anhängt (`--settings <Pfad>`, `--resume <uuid>`).
const LAUNCH_EXTRA: usize = 300;

/// Vorschau der Kommandozeile (einklappbar) und Warnung bei einer zu langen Zeile (cmd-Grenze, `claude.cmd`).
pub(super) fn claude_preview(ui: &mut Ui, lang: Lang, salt: &str, o: &ClaudeOpts, fallback: Option<&ShellSpec>) {
    use crate::claude_opts::{MAX_COMMAND_LEN, WARN_COMMAND_LEN};
    let (spec, legacy): (Option<ShellSpec>, String) = ui.data(|d| d.get_temp(preview_target_id(salt))).unwrap_or_else(|| (fallback.cloned(), String::new()));
    let line = preview_line(o, spec.as_ref(), &legacy);
    let len = line.chars().count() + LAUNCH_EXTRA;
    if len > WARN_COMMAND_LEN {
        let c = if len > MAX_COMMAND_LEN { RED() } else { AMBER() };
        let text = if lang == Lang::De {
            format!("⚠ Die Befehlszeile wird etwa {len} Zeichen lang; über {MAX_COMMAND_LEN} (Grenze von cmd, über die claude.cmd läuft) startet Paneldeck Claude nicht. Kürze den System-Prompt oder die Listen.")
        } else {
            format!("⚠ The command line will be about {len} characters long; above {MAX_COMMAND_LEN} (the limit of cmd, which claude.cmd runs through) Paneldeck does not start Claude. Shorten the system prompt or the lists.")
        };
        let r = hint_label(ui, &text, c);
        dlg_mark(ui, "cmd_len", &r);
    }
    super::opts_tabs::preview(ui, lang, salt, || line);
}

/// Alle Claude-Optionen in einem Raster ohne Reiter (für Tests und Stellen ohne Platzproblem). `false`, solange ein Wert
/// ungültig ist.
#[cfg(test)]
pub(super) fn claude_opts_rows(ui: &mut Ui, lang: Lang, o: &mut ClaudeOpts, salt: &str, place: OptsPlace, target: &folder_picker::Target) -> bool {
    let chk = check_opts(ui.ctx(), lang, o, salt, place, target);
    claude_basic_rows(ui, lang, o, salt, place);
    claude_access_rows(ui, lang, o, salt, place, target, &chk);
    claude_advanced_rows(ui, lang, o, salt, place, &chk);
    check_opts(ui.ctx(), lang, o, salt, place, target).ok()
}

/// Breite der Hinweistexte in den Dialogen (fester Umbruch, damit das Raster nicht von Bild zu Bild springt, #241).
pub(super) const HINT_W: f32 = 440.0;

/// Platz, den Titel, Knöpfe und Rand eines Dialogs außer der Scrollfläche brauchen.
const DLG_CHROME: f32 = 170.0;

/// Höchsthöhe der Scrollfläche eines Dialogs: die Knöpfe darunter bleiben auch auf kleinen Bildschirmen sichtbar (#241).
pub(super) fn dialog_body_height(ctx: &egui::Context) -> f32 {
    (ctx.content_rect().height() - DLG_CHROME).max(120.0)
}

/// Kleiner Hinweistext mit fester Umbruchbreite [`HINT_W`] (auch in einem Raster).
pub(super) fn hint_label(ui: &mut Ui, text: &str, color: Color32) -> egui::Response {
    ui.vertical(|ui| {
        ui.set_max_width(HINT_W);
        ui.add(egui::Label::new(RichText::new(text).small().color(color)).wrap())
    })
    .inner
}

/// Rechteck eines Elements für die Tests ohne Fenster.
fn dlg_mark(ui: &Ui, key: &'static str, r: &egui::Response) {
    #[cfg(test)]
    ui.data_mut(|d| d.insert_temp(Id::new(("dlg_mark", key, 0usize)), r.rect));
    let _ = (ui, key, r);
}

/// Ergebnis eines Bildes des Dialogs „Neue Sitzung“.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NewDlgOut {
    Open,
    Create,
    Cancel,
}

/// Der Dialog „Neue Sitzung“ (ohne App, für Tests ohne Fenster). Inhalt in einer Scrollfläche mit Höchsthöhe nach dem
/// Bildschirm, „Erstellen“/„Abbrechen“ immer darunter sichtbar (#241). Beim Öffnen bekommt das Namensfeld den Fokus
/// (nicht das Terminal dahinter, #255); Esc wird verbraucht und gilt nicht bei offener Auswahlliste (#261).
#[allow(clippy::too_many_arguments)]
pub(super) fn new_dialog_ui(
    ctx: &egui::Context,
    d: &mut NewDialog,
    shells: &[ShellSpec],
    profiles: &[Profile],
    claude_mode: bool,
    wsl_recent: &std::collections::BTreeMap<String, Vec<String>>,
    target: folder_picker::Target,
    lang: Lang,
) -> NewDlgOut {
    use super::opts_tabs::{self as tabs, Tab, TabInfo};
    const SALT: &str = "newdlg";
    let mut out = NewDlgOut::Open;
    let mut can_create = false;
    // Esc schließt den Dialog nicht, solange eine Auswahlliste offen ist (die schließt Esc zuerst), und wird
    // verbraucht – kein Terminal dahinter bekommt es (#261, wie in den Einstellungen der Sitzung).
    let esc = !egui::Popup::is_any_open(ctx) && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
    // Reiter (#292): öffnet immer auf „Allgemein“ (Fokus auf dem Namen); Strg+Tab, Alt+1 … wechseln; Enter erstellt.
    // Ein gewählter Tag-Vorschlag nimmt Enter selbst (#239).
    let win = Id::new("new_session_dialog");
    if d.focus {
        tabs::set(ctx, SALT, Tab::General);
        tabs::activate(ctx, SALT);
    }
    let enter = !super::planning_ui::value_suggest::wants_enter(ctx, Id::new("newdlg_tags")) && tabs::enter_confirms(ctx, SALT, win);
    let tab_list = tabs::session_tabs(d.claude);
    tabs::handle_keys(ctx, SALT, win, &tab_list);
    let defaults = user_defaults(ctx);
    let width = tabs::dialog_width(ctx);
    // Platz für Reiterleiste, Vorschau der Kommandozeile und Abstände (Titel und Knöpfe zählt `dialog_body_height`).
    let body_h = dialog_body_height(ctx) - 70.0;
    let title = if d.claude { lang.t("Neue Claude-Sitzung", "New Claude session") } else { lang.t("Neue Sitzung", "New session") };
    // Verschiebbar an der Titelleiste, öffnet mittig, bleibt im Bild (#288).
    style::movable(ctx, win, egui::Window::new(title))
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.set_width(width);
            let chk = check_opts(ctx, lang, &d.copts, SALT, OptsPlace::NewSession, &target);
            let mut infos = vec![TabInfo::plain(Tab::General)];
            if d.claude {
                infos.extend(claude_tab_infos(&d.copts, &defaults, &chk));
            }
            let tab = tabs::strip(ui, lang, SALT, &infos);
            let invalid = tabs::invalid_hint(lang, &infos);
            tabs::body(ui, SALT, tab, body_h, |ui| match tab {
                Tab::General => new_dialog_general(ui, d, shells, profiles, claude_mode, wsl_recent, &target, lang),
                t => {
                    let NewDialog { copts, conv, prompt, .. } = d;
                    claude_tab(
                        ui,
                        lang,
                        copts,
                        SALT,
                        OptsPlace::NewSession,
                        &target,
                        &defaults,
                        &chk,
                        t,
                        |ui| {
                            // Eine Auswahl statt zweier Häkchen (#241, #245): wie die Konversation beginnt und ob sie beim
                            // Neu laden/Neustart fortgesetzt wird.
                            ui.label(lang.t("Fortsetzen nach Neustart", "Resume after restart"));
                            let r = session_settings::conv_combo(ui, "newdlg_conv", conv, lang);
                            dlg_mark(ui, "conv", &r);
                            ui.end_row();
                        },
                        |ui| {
                            ui.label(lang.t("Start-Prompt", "Start prompt"));
                            ui.add(egui::TextEdit::singleline(prompt).desired_width(FIELD_W));
                            ui.end_row();
                        },
                    );
                }
            });
            let opts_ok = !d.claude || check_opts(ctx, lang, &d.copts, SALT, OptsPlace::NewSession, &target).ok();
            if d.claude {
                claude_preview(ui, lang, SALT, &d.copts, shells.iter().find(|s| s.id == d.shell_id));
            }
            ui.add_space(6.0);
            // Ungültiger oder fehlender WSL-Ordner: Erstellen gesperrt (Grund steht unter dem Ordnerfeld).
            let blocked = d.picker.blocking_error(lang);
            can_create = opts_ok && blocked.is_none();
            ui.horizontal(|ui| {
                let b = ui.add_enabled(can_create, egui::Button::new(lang.t("Erstellen", "Create")));
                dlg_mark(ui, "create", &b);
                if b.clicked() {
                    out = NewDlgOut::Create;
                }
                if let Some(e) = &blocked {
                    b.clone().on_disabled_hover_text(e.as_str());
                }
                let c = ui.button(lang.t("Abbrechen", "Cancel"));
                dlg_mark(ui, "cancel", &c);
                if c.clicked() {
                    out = NewDlgOut::Cancel;
                }
                // Ungültiger Wert in einem anderen Reiter: sagen, wo (#292).
                tabs::show_invalid(ui, &invalid, &b);
                #[cfg(test)]
                ui.data_mut(|m| m.insert_temp(Id::new("newdlg_invalid"), invalid.clone()));
            });
        });
    if enter && can_create && out == NewDlgOut::Open {
        out = NewDlgOut::Create;
    }
    if esc && out == NewDlgOut::Open {
        out = NewDlgOut::Cancel;
    }
    out
}

/// Anlage-Optionen aus dem Dialog „Neue Sitzung“ (mit Farbe, Tags und Notizen, #292).
fn new_dialog_opts(d: &NewDialog) -> NewOpts {
    NewOpts {
        shell_id: Some(d.shell_id.clone()),
        cwd: Some(d.cwd.clone()),
        name: Some(d.name.clone()),
        claude: d.claude,
        claude_opts: Some(d.copts.sanitized(true)),
        allow_bypass: true,
        continue_last: d.conv.continue_last(),
        resume_mode: Some(d.conv.mode()),
        prompt: Some(d.prompt.clone()),
        profile: Some(d.profile),
        group: Some(d.group.clone()),
        color: d.own_color.then_some(d.color),
        tags: crate::templates::TplItem::clean_tags(&session_settings::parse_tags(&d.tags)),
        notes: d.notes.trim_end().to_string(),
        ..NewOpts::default()
    }
}

/// Reiter *Allgemein* des Dialogs „Neue Sitzung“: Name, Claude starten, Profil, Shell, Ordner, Ordner im Baum, Worktree.
#[allow(clippy::too_many_arguments)]
fn new_dialog_general(ui: &mut Ui, d: &mut NewDialog, shells: &[ShellSpec], profiles: &[Profile], claude_mode: bool, wsl_recent: &std::collections::BTreeMap<String, Vec<String>>, target: &folder_picker::Target, lang: Lang) {
    egui::Grid::new("newgrid").num_columns(2).spacing([8.0, 8.0]).min_col_width(140.0).show(ui, |ui| {
        ui.label(lang.t("Name (optional)", "Name (optional)"));
        let te = ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(FIELD_W));
        if d.focus {
            // Beim Öffnen: Tasten gehören dem Dialog, nicht dem Terminal dahinter (#255).
            te.request_focus();
            d.focus = false;
        }
        dlg_mark(ui, "name", &te);
        ui.end_row();
        if claude_mode {
            ui.label("");
            let r = ui.checkbox(&mut d.claude, lang.t("Claude Code starten", "Start Claude Code"));
            dlg_mark(ui, "claude", &r);
            ui.end_row();
        }
        if !profiles.is_empty() {
            ui.label(lang.t("Profil", "Profile"));
            let cur = profiles.iter().find(|p| p.id == d.profile).map(|p| p.name.clone()).unwrap_or_else(|| lang.t("Keins", "None").to_string());
            egui::ComboBox::from_id_salt("newprof").selected_text(cur).show_ui(ui, |ui| {
                if ui.selectable_label(d.profile == 0, lang.t("Keins", "None")).clicked() {
                    d.profile = 0;
                }
                for p in profiles {
                    if ui.selectable_label(d.profile == p.id, &p.name).clicked() {
                        d.profile = p.id;
                        if !p.shell_id.is_empty() {
                            d.shell_id = p.shell_id.clone();
                        }
                        if !p.cwd.is_empty() {
                            d.set_cwd(&p.cwd);
                        }
                    }
                }
            });
            ui.end_row();
        }
        ui.label("Shell");
        // WSL mit erkannter Login-Shell, z. B. „WSL: Ubuntu (zsh)“ (#57).
        let cur = shells.iter().find(|s| s.id == d.shell_id).map(|s| crate::shells::display_label(s, lang)).unwrap_or_default();
        egui::ComboBox::from_id_salt("shellsel").selected_text(cur).show_ui(ui, |ui| {
            for s in shells {
                ui.selectable_value(&mut d.shell_id, s.id.clone(), crate::shells::display_label(s, lang));
            }
        });
        ui.end_row();
        ui.label(lang.t("Ordner", "Folder"));
        // WSL-Shell: Linux-Pfad mit Vervollständigung, Distribution wählbar (wechselt die Shell), #172.
        d.picker.set_target(target.clone());
        let r = d.picker.show(ui, lang, wsl_recent);
        if let Some(sid) = r.distro_changed.and_then(|dist| shells.iter().find(|s| s.id.strip_prefix("wsl:").is_some_and(|x| x.eq_ignore_ascii_case(&dist)))).map(|s| s.id.clone()) {
            d.shell_id = sid;
        }
        d.cwd = d.picker.value();
        ui.end_row();
        ui.label(lang.t("Ordner im Baum", "Tree folder"));
        ui.add(egui::TextEdit::singleline(&mut d.group).hint_text(lang.t("z. B. Backend/API (leer = keiner)", "e.g. Backend/API (empty = none)")).desired_width(FIELD_W));
        ui.end_row();
        // Farbe, Tags, Notizen (#292) wie in den Einstellungen der Sitzung.
        ui.label(lang.t("Farbe", "Color"));
        ui.vertical(|ui| {
            let r = ui.checkbox(&mut d.own_color, lang.t("Eigene Farbe", "Own color")).on_hover_text(lang.t("Aus: Farbe des Profils bzw. automatisch", "Off: the profile's colour or automatic"));
            dlg_mark(ui, "own_color", &r);
            if d.own_color {
                session_settings::color_row(ui, &mut d.color, lang);
            }
        });
        ui.end_row();
        ui.label(lang.t("Tags", "Tags"));
        let r = super::planning_ui::value_suggest::suggest_edit(ui, Id::new("newdlg_tags"), &mut d.tags, &d.tag_values, &[], super::planning_ui::value_suggest::Mode::List, lang, |ui, te| ui.add(te.hint_text(lang.t("mit Komma getrennt", "comma separated")).desired_width(FIELD_W)));
        let _ = r;
        ui.end_row();
        ui.label(lang.t("Notizen", "Notes"));
        let r = super::opts_tabs::notes_edit(ui, "newdlg", &mut d.notes, FIELD_W);
        dlg_mark(ui, "notes", &r);
        ui.end_row();
        if d.claude {
            // Git-Worktree anbieten, wenn der Ordner in einem Repository liegt (Prüfung nur bei Änderung des Pfads,
            // ohne git-Prozess pro Tastendruck; das echte Wurzelverzeichnis ermittelt erst das Anlegen).
            if d.repo_for != d.cwd {
                d.repo_for = d.cwd.clone();
                let p = std::path::Path::new(&d.cwd);
                // Linux-Pfad (WSL): kein Worktree-Angebot (git liefe in Windows auf einem fremden Pfad).
                // Ebenso kein Prüfen eines \\wsl$-Pfads im UI-Thread.
                d.repo = (!d.picker.is_wsl() && !crate::wsl::is_wsl_unc(&d.cwd) && gitops::inside_repo_fs(p)).then(|| p.to_path_buf());
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
                    hint_label(ui, lang.t("Legt einen neuen Branch und Ordner neben dem Repo an, damit parallele Sitzungen sich nicht stören.", "Creates a new branch and folder next to the repo so parallel sessions don't interfere."), style::dim(ui));
                    ui.end_row();
                }
            }
        }
    });
}

#[cfg(test)]
mod layout_tests {
    //! #241: Dialoge passen auf kleine Bildschirme, die Knöpfe bleiben sichtbar, Hinweise überdecken nichts.
    use super::*;
    use eframe::egui::{Event, RawInput};

    fn shells() -> Vec<ShellSpec> {
        vec![ShellSpec { id: "pwsh".into(), label: "PowerShell".into(), program: "pwsh.exe".into(), args: vec![] }, ShellSpec { id: "wsl:Ubuntu".into(), label: "Ubuntu".into(), program: "wsl.exe".into(), args: vec![] }]
    }

    pub(super) fn mark(ctx: &egui::Context, key: &'static str, n: usize) -> Option<Rect> {
        ctx.data(|d| d.get_temp::<Rect>(Id::new(("dlg_mark", key, n))))
    }

    /// Dialog mit allem, was ihn groß macht: bypassPermissions- und safe-mode-Warnung, alle weiteren Optionen offen,
    /// viele Ordner, Worktree mit Hinweis.
    fn big_dialog() -> NewDialog {
        // Eigenes, frisches Repository als Startordner: unabhängig davon, ob die Tests in einem Git-Checkout laufen.
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let repo = std::env::temp_dir().join(format!("pd-dlg-repo-{}-{n}", std::process::id()));
        if !repo.join(".git").exists() {
            std::fs::create_dir_all(&repo).unwrap();
            crate::gitops::git(&repo, &["init", "-q"]).expect("git init");
        }
        let here = repo.to_string_lossy().to_string();
        let mut copts = ClaudeOpts { permission_mode: crate::claude_opts::BYPASS.into(), ..ClaudeOpts::default() };
        copts.extra.flags = vec!["chrome".into(), "ide".into(), "verbose".into(), "safe-mode".into()];
        copts.extra.add_dirs = (0..6).map(|i| format!("C:\\gibt-es-nicht\\ordner-{i}")).collect();
        copts.extra.plugin_dirs = (0..4).map(|i| format!("C:\\gibt-es-nicht\\plugin-{i}")).collect();
        copts.extra.mcp_configs = (0..4).map(|i| format!("C:\\gibt-es-nicht\\mcp-{i}.json")).collect();
        copts.extra.strict_mcp_config = true;
        copts.extra.allowed_tools = "Read".into();
        copts.extra.disallowed_tools = "WebFetch".into();
        copts.extra.system_prompt = "You are a careful reviewer.\nAlways explain.\nNever guess.".into();
        copts.extra.append_system_prompt = "Be brief.".into();
        copts.extra.fallback_model = "sonnet".into();
        copts.extra.max_budget_usd = "5".into();
        copts.extra.autocompact = "200k".into();
        copts.model = "opus".into();
        copts.effort = "high".into();
        copts.agent = "reviewer".into();
        let mut d = NewDialog::new("pwsh".into(), true, copts);
        d.set_cwd(&here);
        d.worktree = true;
        d.branch = "claude/1".into();
        d
    }

    fn frame(ctx: &egui::Context, d: &mut NewDialog, size: Vec2, t: f64, events: Vec<Event>) -> (NewDlgOut, Vec<Key>) {
        let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), time: Some(t), events, ..Default::default() };
        let (mut out, mut left) = (NewDlgOut::Open, Vec::new());
        let s = shells();
        let target = folder_picker::Target::Windows;
        let _ = ctx.run(input, |ctx| {
            ctx.data_mut(|m| m.insert_temp(Id::new("dlg_hint_n"), 0usize));
            out = new_dialog_ui(ctx, d, &s, &[], true, &Default::default(), target.clone(), Lang::En);
            left = ctx.input(|i| i.events.iter().filter_map(|e| if let Event::Key { key, pressed: true, .. } = e { Some(*key) } else { None }).collect());
        });
        (out, left)
    }

    fn key(k: Key, m: Modifiers) -> Event {
        Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: m }
    }

    #[test]
    fn the_new_session_dialog_fits_the_screen_on_every_tab() {
        use super::super::opts_tabs::{self as tabs, Tab};
        // #292: mit allem gefüllt kein Scrollen bei 1280×720 und 1920×1080, bei 800×600 scrollt es, die Knöpfe bleiben
        // sichtbar (#241); jeder Reiter gleich hoch.
        for size in [Vec2::new(1920.0, 1080.0), Vec2::new(1280.0, 720.0), Vec2::new(800.0, 600.0)] {
            let ctx = egui::Context::default();
            let mut d = big_dialog();
            let screen = Rect::from_min_size(Pos2::ZERO, size);
            let mut heights = Vec::new();
            for (n, tab) in [Tab::General, Tab::Claude, Tab::Access, Tab::Advanced].into_iter().enumerate() {
                if n > 0 {
                    tabs::set(&ctx, "newdlg", tab);
                }
                for i in 0..6 {
                    frame(&ctx, &mut d, size, (n * 10 + i) as f64 * 0.1, vec![]);
                }
                assert_eq!(tabs::current(&ctx, "newdlg", &tabs::session_tabs(true)), tab);
                let win = ctx.memory(|m| m.area_rect(Id::new("new_session_dialog"))).expect("Fenster gezeichnet");
                assert!(screen.contains_rect(win), "{size:?} {tab:?}: Fenster {win:?} ragt aus dem Bildschirm");
                assert!(win.width() <= tabs::DIALOG_W + 40.0, "{size:?}: {win:?}");
                heights.push(win.height());
                for k in ["create", "cancel"] {
                    let r = mark(&ctx, k, 0).unwrap_or_else(|| panic!("{k} fehlt"));
                    assert!(screen.contains_rect(r) && win.contains_rect(r), "{size:?} {tab:?}: {k} {r:?} nicht sichtbar");
                }
                if size.y >= 720.0 {
                    assert!(!tabs::overflowed(&ctx, "newdlg"), "{size:?} {tab:?}: der Reiter müsste scrollen");
                }
                if tab == Tab::Advanced {
                    // Die Warnung zu --safe-mode überdeckt die Schalter nicht
                    let sw = mark(&ctx, "switches", 0).expect("Schalter gezeichnet");
                    let hint = mark(&ctx, "more_hint", 0).expect("Warnung gezeichnet");
                    assert!(!sw.intersects(hint.shrink(1.0)), "{size:?}: Warnung {hint:?} über den Schaltern {sw:?}");
                    assert!(hint.width() <= HINT_W + 1.0, "fester Umbruch: {hint:?}");
                }
            }
            assert!(heights.iter().all(|h| (h - heights[0]).abs() < 2.0), "{size:?}: alle Reiter gleich hoch: {heights:?}");
            assert!(d.repo.is_some(), "der Testordner liegt in einem Repository");
        }
    }

    #[test]
    fn tabs_counts_marks_keys_and_enter() {
        use super::super::opts_tabs::{self as tabs, Mark, Tab};
        let ctx = egui::Context::default();
        let size = Vec2::new(1280.0, 720.0);
        let mut d = big_dialog();
        frame(&ctx, &mut d, size, 0.0, vec![]);
        frame(&ctx, &mut d, size, 0.1, vec![]);
        let tab = |n: usize| ctx.data(|m| m.get_temp::<(Rect, String, Mark)>(Id::new(("tab_mark", "newdlg".to_string(), n)))).expect("Reiter");
        assert_eq!(tab(0).1, "General");
        assert_eq!(tab(1).1, "Claude (4)", "Modell, Effort, Permission-Mode, Agent weichen von den Standards ab");
        assert!(tab(2).1.starts_with("Access (") && tab(2).2 == Mark::Error, "fehlende Ordner: rot {:?}", tab(2));
        assert_eq!(tab(1).2, Mark::Warn, "bypassPermissions: orange");
        assert_eq!(tab(3).2, Mark::Warn, "eigener System-Prompt und --safe-mode: orange");
        // Ohne Claude nur „Allgemein“.
        let mut plain = NewDialog::new("pwsh".into(), false, ClaudeOpts::default());
        let c2 = egui::Context::default();
        frame(&c2, &mut plain, size, 0.0, vec![]);
        assert!(c2.data(|m| m.get_temp::<(Rect, String, Mark)>(Id::new(("tab_mark", "newdlg".to_string(), 1usize)))).is_none());
        // Tasten: Strg+Tab, Strg+Umschalt+Tab, Alt+Zahl – verbraucht.
        let (_, left) = frame(&ctx, &mut d, size, 0.2, vec![key(Key::Tab, Modifiers::CTRL)]);
        assert_eq!(tabs::current(&ctx, "newdlg", &tabs::session_tabs(true)), Tab::Claude);
        assert!(left.is_empty(), "{left:?}");
        frame(&ctx, &mut d, size, 0.3, vec![key(Key::Num4, Modifiers::ALT)]);
        assert_eq!(tabs::current(&ctx, "newdlg", &tabs::session_tabs(true)), Tab::Advanced);
        frame(&ctx, &mut d, size, 0.4, vec![key(Key::Tab, Modifiers::CTRL | Modifiers::SHIFT)]);
        assert_eq!(tabs::current(&ctx, "newdlg", &tabs::session_tabs(true)), Tab::Access);
        // Zurücksetzen auf die Standards (hier leer): der Reiter zählt nichts mehr.
        let mut o = d.copts.clone();
        reset_tab(&mut o, &ClaudeOpts::default(), Tab::Access);
        assert_eq!(tab_counts(&o, &ClaudeOpts::default())[1], 0);
        assert!(o.extra.add_dirs.is_empty() && o.extra.mcp_configs.is_empty() && o.model == "opus", "nur der Reiter");
    }

    #[test]
    fn colour_tags_and_notes_reach_the_new_session() {
        // #292: im Reiter „Allgemein“ und in den Optionen der neuen Sitzung.
        let ctx = egui::Context::default();
        let size = Vec2::new(1280.0, 720.0);
        let mut d = NewDialog::new("pwsh".into(), false, ClaudeOpts::default());
        frame(&ctx, &mut d, size, 0.0, vec![]);
        frame(&ctx, &mut d, size, 0.1, vec![]);
        assert!(mark(&ctx, "own_color", 0).is_some() && mark(&ctx, "notes", 0).is_some());
        let o = new_dialog_opts(&d);
        assert_eq!((o.color, o.tags.len(), o.notes.as_str()), (None, 0, ""), "ohne Angaben: Farbe automatisch");
        d.own_color = true;
        d.color = [1, 2, 3];
        d.tags = " web, #api ,web".into();
        d.notes = "line 1\nline 2\n".into();
        frame(&ctx, &mut d, size, 0.2, vec![]);
        let o = new_dialog_opts(&d);
        assert_eq!(o.color, Some([1, 2, 3]));
        assert_eq!(o.tags, ["web", "api"], "ohne Doppelte");
        assert_eq!(o.notes, "line 1\nline 2");
        assert_eq!(session_color(o.color, Some([9, 9, 9]), [0, 0, 0]), [1, 2, 3], "eigene Farbe vor der des Profils");
        assert_eq!(session_color(None, Some([9, 9, 9]), [0, 0, 0]), [9, 9, 9]);
        assert_eq!(session_color(None, None, [7, 7, 7]), [7, 7, 7]);
    }

    #[test]
    fn enter_creates_from_every_tab() {
        use super::super::opts_tabs::{self as tabs, Tab};
        let size = Vec2::new(1280.0, 720.0);
        for tab in [Tab::General, Tab::Claude, Tab::Access, Tab::Advanced] {
            let ctx = egui::Context::default();
            let mut d = NewDialog::new("pwsh".into(), true, ClaudeOpts::default());
            frame(&ctx, &mut d, size, 0.0, vec![]);
            tabs::set(&ctx, "newdlg", tab);
            frame(&ctx, &mut d, size, 0.1, vec![]);
            frame(&ctx, &mut d, size, 0.2, vec![]);
            let (o, _) = frame(&ctx, &mut d, size, 0.3, vec![key(Key::Enter, Modifiers::NONE)]);
            assert_eq!(o, NewDlgOut::Create, "{tab:?}");
        }
        // Ungültige Werte: Enter erstellt nicht; neben „Erstellen“ steht, in welchem Reiter.
        let ctx = egui::Context::default();
        let mut d = NewDialog::new("pwsh".into(), true, ClaudeOpts { model: "a b".into(), ..ClaudeOpts::default() });
        frame(&ctx, &mut d, size, 0.0, vec![]);
        let (o, _) = frame(&ctx, &mut d, size, 0.1, vec![key(Key::Enter, Modifiers::NONE)]);
        assert_eq!(o, NewDlgOut::Open);
        let hint = ctx.data(|m| m.get_temp::<Option<String>>(Id::new("newdlg_invalid"))).flatten();
        assert_eq!(hint.as_deref(), Some("Invalid value in tab “Claude”"));
        // Klick neben den Dialog (z. B. ins Terminal): Enter gehört nicht mehr dem Dialog.
        let ctx = egui::Context::default();
        let mut d = NewDialog::new("pwsh".into(), true, ClaudeOpts::default());
        frame(&ctx, &mut d, size, 0.0, vec![]);
        let p = Pos2::new(5.0, 5.0);
        frame(&ctx, &mut d, size, 0.1, vec![Event::PointerMoved(p), Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
        frame(&ctx, &mut d, size, 0.2, vec![Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
        let (o, left) = frame(&ctx, &mut d, size, 0.3, vec![key(Key::Enter, Modifiers::NONE)]);
        assert_eq!(o, NewDlgOut::Open, "nicht im Dialog");
        assert_eq!(left, vec![Key::Enter], "bleibt für das Terminal");
    }

    #[test]
    fn the_new_session_dialog_takes_focus_and_eats_escape() {
        let ctx = egui::Context::default();
        let mut d = NewDialog::new("pwsh".into(), true, ClaudeOpts::default());
        let size = Vec2::new(1000.0, 700.0);
        frame(&ctx, &mut d, size, 0.0, vec![]);
        assert!(!d.focus, "Fokus beim ersten Bild vergeben");
        let name = mark(&ctx, "name", 0).unwrap();
        frame(&ctx, &mut d, size, 0.1, vec![]);
        assert!(ctx.memory(|m| m.focused()).is_some(), "ein Feld des Dialogs hat den Fokus");
        // Tippen landet im Namensfeld
        frame(&ctx, &mut d, size, 0.2, vec![Event::Text("abc".into())]);
        assert_eq!(d.name, "abc");
        let _ = name;
        // Esc schließt und wird verbraucht
        let esc = Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() };
        let (o, left) = frame(&ctx, &mut d, size, 0.3, vec![esc]);
        assert_eq!(o, NewDlgOut::Cancel);
        assert!(left.is_empty(), "{left:?}");
    }

    #[test]
    fn one_conversation_control_maps_onto_the_launch() {
        use session_settings::ConvChoice;
        let ctx = egui::Context::default();
        let mut d = NewDialog::new("pwsh".into(), true, ClaudeOpts::default());
        assert_eq!(d.conv, ConvChoice::Resume, "Standard wie bisher: erster Start neu, beim Neu laden fortsetzen");
        frame(&ctx, &mut d, Vec2::new(1000.0, 700.0), 0.0, vec![]);
        super::super::opts_tabs::set(&ctx, "newdlg", super::super::opts_tabs::Tab::Claude);
        frame(&ctx, &mut d, Vec2::new(1000.0, 700.0), 0.1, vec![]);
        assert!(mark(&ctx, "conv", 0).is_some(), "eine Auswahl im Dialog (Reiter „Claude“)");
        use crate::state::ResumeMode;
        assert_eq!((ConvChoice::Resume.continue_last(), ConvChoice::Resume.mode()), (false, ResumeMode::Continue));
        assert_eq!((ConvChoice::ContinueLast.continue_last(), ConvChoice::ContinueLast.mode()), (true, ResumeMode::Continue));
        assert_eq!((ConvChoice::Pick.continue_last(), ConvChoice::Pick.mode()), (false, ResumeMode::Pick));
        assert_eq!((ConvChoice::AlwaysNew.continue_last(), ConvChoice::AlwaysNew.mode()), (false, ResumeMode::Off));
        for c in ConvChoice::ALL {
            assert_eq!(ConvChoice::of(c.mode(), c.continue_last()), c);
            assert_ne!(c.label(Lang::De), c.label(Lang::En));
            assert_ne!(c.tip(Lang::De), c.tip(Lang::En));
        }
    }

    /// #288: Der Dialog öffnet mittig, lässt sich an der Titelleiste verschieben, behält die Position, solange er offen
    /// ist, bleibt im Bildschirm und öffnet nach dem Schließen wieder mittig.
    #[test]
    fn the_new_session_dialog_can_be_dragged_by_its_title_bar_and_stays_on_screen() {
        let ctx = egui::Context::default();
        let mut d = NewDialog::new("pwsh".into(), true, ClaudeOpts::default());
        let size = Vec2::new(1200.0, 800.0);
        let screen = Rect::from_min_size(Pos2::ZERO, size);
        let mut t = 0.0;
        let mut step = |d: &mut NewDialog, ev: Vec<Event>| {
            t += 0.1;
            frame(&ctx, d, size, t, ev).0
        };
        let win = || ctx.memory(|m| m.area_rect(Id::new("new_session_dialog"))).expect("Fenster gezeichnet");
        let btn = |pos: Pos2, pressed: bool| Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        // Ziehen über mehrere Bilder (Drücken, Bewegen, Loslassen) wie mit der Maus.
        let drag = |step: &mut dyn FnMut(&mut NewDialog, Vec<Event>) -> NewDlgOut, d: &mut NewDialog, from: Pos2, to: Pos2| {
            assert_eq!(step(d, vec![Event::PointerMoved(from)]), NewDlgOut::Open);
            assert_eq!(step(d, vec![btn(from, true)]), NewDlgOut::Open);
            step(d, vec![Event::PointerMoved(from.lerp(to, 0.5))]);
            step(d, vec![Event::PointerMoved(to)]);
            assert_eq!(step(d, vec![btn(to, false)]), NewDlgOut::Open, "Ziehen schließt den Dialog nicht");
            step(d, vec![]);
        };
        for _ in 0..4 {
            step(&mut d, vec![]);
        }
        let start = win();
        assert!((start.center() - screen.center()).length() < 2.0, "öffnet mittig: {start:?}");
        // Titelleiste: oberer Rand des Fensters, rechts vom Titeltext (kein Knopf dort).
        let grip = Pos2::new(start.max.x - 30.0, start.min.y + 10.0);
        let delta = Vec2::new(-150.0, -90.0);
        drag(&mut step, &mut d, grip, grip + delta);
        let moved = win();
        assert!((moved.min - (start.min + delta)).length() < 1.5, "verschoben um {delta:?}: {start:?} → {moved:?}");
        assert_eq!(moved.size(), start.size(), "Größe bleibt");
        // Solange der Dialog offen ist, bleibt er dort.
        for _ in 0..3 {
            step(&mut d, vec![]);
        }
        assert_eq!(win(), moved, "Position bleibt, solange der Dialog offen ist");
        // Weit über den Rand gezogen: bleibt im Bildschirm.
        let grip = Pos2::new(moved.max.x - 30.0, moved.min.y + 10.0);
        drag(&mut step, &mut d, grip, grip + Vec2::new(-3000.0, -3000.0));
        let edge = win();
        assert!(screen.contains_rect(edge), "bleibt im Bildschirm: {edge:?}");
        assert!(edge.min.x <= 1.0 && edge.min.y <= 1.0, "am linken oberen Rand: {edge:?}");
        let grip = Pos2::new(edge.max.x - 30.0, edge.min.y + 10.0);
        drag(&mut step, &mut d, grip, grip + Vec2::new(5000.0, 5000.0));
        // (egui rundet auf Pixel; Rahmenstrich kann Bruchteile überstehen)
        assert!(screen.expand(0.5).contains_rect(win()), "bleibt im Bildschirm: {:?}", win());
        assert!(win().max.x >= size.x - 1.0 && win().max.y >= size.y - 1.0, "am rechten unteren Rand: {:?}", win());
        // Ein Bild ohne Dialog (geschlossen), dann wieder öffnen: wieder mittig.
        t += 0.1;
        let _ = ctx.run(RawInput { screen_rect: Some(screen), time: Some(t), ..Default::default() }, |_| {});
        let mut d = NewDialog::new("pwsh".into(), true, ClaudeOpts::default());
        for _ in 0..3 {
            t += 0.1;
            frame(&ctx, &mut d, size, t, vec![]);
        }
        assert!((win().center() - screen.center()).length() < 2.0, "öffnet wieder mittig: {:?}", win());
    }
}

#[cfg(test)]
mod copts_tests {
    use super::*;
    use crate::claude_opts::ExtraOpts;

    /// Die Zeilen ohne Fenster zeichnen (headless) und das Ergebnis der Prüfung liefern.
    fn rows(o: &mut ClaudeOpts) -> bool {
        rows_with(o, OptsPlace::NewSession)
    }

    fn rows_with(o: &mut ClaudeOpts, place: OptsPlace) -> bool {
        rows_ctx(&egui::Context::default(), o, place)
    }

    fn rows_ctx(ctx: &egui::Context, o: &mut ClaudeOpts, place: OptsPlace) -> bool {
        let mut ok = None;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                egui::Grid::new("t").num_columns(2).show(ui, |ui| {
                    ok = Some(claude_opts_rows(ui, Lang::En, o, "t", place, &folder_picker::Target::Windows));
                });
            });
        });
        ok.unwrap()
    }

    fn extra(e: ExtraOpts) -> ClaudeOpts {
        ClaudeOpts { extra: e, ..Default::default() }
    }

    #[test]
    fn more_options_section() {
        let ctx = egui::Context::default();
        assert!(rows_ctx(&ctx, &mut ClaudeOpts::default(), OptsPlace::NewSession));
        let d = std::env::temp_dir().join(format!("pd-flags-ui-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("mcp.json");
        std::fs::write(&f, "{}").unwrap();
        let (dir, file) = (d.to_string_lossy().to_string(), f.to_string_lossy().to_string());
        let mut o = extra(ExtraOpts {
            flags: vec!["chrome".into(), "ide".into(), "safe-mode".into()],
            add_dirs: vec![dir.clone()],
            plugin_dirs: vec![dir.clone()],
            allowed_tools: "Bash(git log:*) Read".into(),
            disallowed_tools: "WebFetch".into(),
            mcp_configs: vec![file.clone()],
            strict_mcp_config: true,
            system_prompt: "You review code.\nBe strict.".into(),
            append_system_prompt: "Be brief. ".into(),
            fallback_model: "sonnet".into(),
            max_budget_usd: "2.50".into(),
            autocompact: "200k".into(),
        });
        let ctx = egui::Context::default();
        assert!(rows_ctx(&ctx, &mut o, OptsPlace::NewSession), "gültige Werte");
        assert_eq!(o.extra.flags, ["chrome", "ide", "safe-mode"], "safe-mode ist ein Schalter für den Nutzer");
        assert_eq!(o.extra.add_dirs, [dir.clone()]);
        assert!(o.extra.strict_mcp_config, "mit MCP-Konfiguration wählbar");
        assert_eq!(o.extra.append_system_prompt, "Be brief. ", "Text wird beim Tippen nicht beschnitten");
        assert_eq!(o.sanitized(true), o.trimmed(), "gültig = bereinigt gleich getrimmt");
        // Standards (Einstellungen): dieselben Felder, auch die Rechte erweiternden.
        assert!(rows_with(&mut o.clone(), OptsPlace::Defaults));
        assert!(rows_with(&mut o.clone(), OptsPlace::Session));

        // Ungültig: fehlender Ordner, Netzwerkpfad (ohne Zugriff aufs Netz), fehlende Datei, Shell-Zeichen, Budget, Modell.
        let sep = std::path::MAIN_SEPARATOR;
        for bad in [
            ExtraOpts { add_dirs: vec![format!("{dir}{sep}missing")], ..Default::default() },
            ExtraOpts { add_dirs: vec!["\\\\server\\share".into()], ..Default::default() },
            ExtraOpts { add_dirs: vec!["relative".into()], ..Default::default() },
            ExtraOpts { mcp_configs: vec![format!("{dir}{sep}missing.json")], ..Default::default() },
            ExtraOpts { mcp_configs: vec![dir.clone()], ..Default::default() },
            ExtraOpts { plugin_dirs: vec![format!("{dir}{sep}missing")], ..Default::default() },
            ExtraOpts { plugin_dirs: vec!["\\\\server\\plug".into()], ..Default::default() },
            ExtraOpts { system_prompt: "a & b".into(), ..Default::default() },
            ExtraOpts { system_prompt: "ok\n-x|y".into(), ..Default::default() },
            ExtraOpts { allowed_tools: "Bash(x); calc".into(), ..Default::default() },
            ExtraOpts { disallowed_tools: "a\"b".into(), ..Default::default() },
            ExtraOpts { append_system_prompt: "50% & more".into(), ..Default::default() },
            ExtraOpts { append_system_prompt: "-x".into(), ..Default::default() },
            ExtraOpts { fallback_model: "a b".into(), ..Default::default() },
            ExtraOpts { max_budget_usd: "1e3".into(), ..Default::default() },
            ExtraOpts { max_budget_usd: "0".into(), ..Default::default() },
            ExtraOpts { append_system_prompt: "a\\b".into(), ..Default::default() },
            ExtraOpts { autocompact: "sometimes".into(), ..Default::default() },
        ] {
            assert!(!rows(&mut extra(bad.clone())), "{bad:?}");
        }
        // Ein fehlender Ordner sperrt; das Ergebnis bleibt kurz zwischengespeichert.
        let ctx = egui::Context::default();
        let mut missing = extra(ExtraOpts { add_dirs: vec![format!("{dir}{sep}missing")], ..Default::default() });
        assert!(!rows_ctx(&ctx, &mut missing, OptsPlace::NewSession));
        assert!(!rows_ctx(&ctx, &mut missing, OptsPlace::NewSession), "Ergebnis aus dem Zwischenspeicher");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn strict_mcp_config_needs_a_config_and_linux_paths_need_wsl() {
        // Ohne MCP-Konfiguration wird --strict-mcp-config abgewählt (und ist gesperrt).
        let ctx = egui::Context::default();
        let mut o = extra(ExtraOpts { strict_mcp_config: true, ..Default::default() });
        assert!(rows_ctx(&ctx, &mut o, OptsPlace::NewSession));
        assert!(!o.extra.strict_mcp_config);
        // Linux-Pfade: in einer Windows-Sitzung nur eine Warnung (werden beim Start weggelassen; können aus den Standards
        // stammen und dürfen „Erstellen“ nicht sperren), als Standard/in Vorlagen erlaubt (gelten nur in WSL).
        if cfg!(windows) {
            let lin = || extra(ExtraOpts { plugin_dirs: vec!["/home/u/plug".into()], ..Default::default() });
            assert!(rows_with(&mut lin(), OptsPlace::NewSession));
            assert!(rows_with(&mut lin(), OptsPlace::Session));
            let chk = check_opts(&egui::Context::default(), Lang::En, &lin(), "t", OptsPlace::NewSession, &folder_picker::Target::Windows);
            assert!(chk.ok() && chk.linux_ignored == ["/home/u/plug"]);
            let infos = claude_tab_infos(&lin(), &ClaudeOpts::default(), &chk);
            assert_eq!(infos[1].mark, super::super::opts_tabs::Mark::Warn, "orange, nicht rot");
            assert!(rows_with(&mut lin(), OptsPlace::Defaults));
            assert!(rows_with(&mut lin(), OptsPlace::Template));
            let mut ok = None;
            let wsl = folder_picker::Target::Wsl { distro: Some("Ubuntu".into()), choose_distro: false };
            let _ = egui::Context::default().run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    egui::Grid::new("t").num_columns(2).show(ui, |ui| {
                        ok = Some(claude_opts_rows(ui, Lang::En, &mut lin(), "t", OptsPlace::NewSession, &wsl));
                    });
                });
            });
            assert_eq!(ok, Some(true), "WSL-Sitzung: Linux-Pfad gültig");
        }
    }

    #[test]
    fn dialog_rows_validate_names() {
        let mut o = ClaudeOpts { model: " opus ".into(), effort: "high".into(), permission_mode: "bypassPermissions".into(), agent: "rev".into(), ..Default::default() };
        assert!(rows(&mut o));
        assert_eq!(o.model, "opus", "getrimmt");
        assert_eq!(o.permission_mode, "bypassPermissions", "der Nutzer darf es selbst wählen");
        assert!(rows(&mut ClaudeOpts::default()));
        assert!(!rows(&mut ClaudeOpts { model: "opus; calc".into(), ..Default::default() }));
        assert!(!rows(&mut ClaudeOpts { agent: "-x".into(), ..Default::default() }));
        // Standards (Einstellungen): bypassPermissions gibt es dort nicht.
        let mut d = ClaudeOpts { permission_mode: "bypassPermissions".into(), ..Default::default() };
        assert!(rows_with(&mut d, OptsPlace::Defaults));
        assert_eq!(d.permission_mode, "");
        // Vorlagen (#244): ebenso nicht – die Datei kann weitergegeben werden.
        let mut t = ClaudeOpts { permission_mode: "bypassPermissions".into(), ..Default::default() };
        assert!(rows_with(&mut t, OptsPlace::Template));
        assert_eq!(t.permission_mode, "");
    }
}
