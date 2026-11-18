//! Dialoge und kleine Fenster.

use super::*;

impl App {
    pub(super) fn dialogs(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let lang = self.settings.language;
        // Rückfrage „Link öffnen?“ (OSC 8, #183)
        self.link_ask_ui(ctx, self.focused);

        // Neue Sitzung (Inhalt in `new_dialog_ui`, ohne App testbar)
        if let Some(mut d) = self.new_dialog.take() {
            let target = self.new_dialog_target(&d.shell_id, d.profile);
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
                let opts = NewOpts {
                    shell_id: Some(d.shell_id.clone()),
                    cwd: Some(d.cwd.clone()),
                    name: Some(d.name.clone()),
                    claude: d.claude,
                    claude_opts: Some(d.copts.sanitized(true)),
                    allow_bypass: true,
                    continue_last: d.conv.continue_last(),
                    claude_continue: Some(d.conv.resume_later()),
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
        // Einstellungen einer Sitzung und Rückfrage „Neu laden?“ (#169, #170).
        // Die Rückfrage zuerst: sie bekommt Esc/Enter, bevor der Einstellungsdialog darunter sie sieht.
        self.reload_ask_ui(ctx, actions, None);
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
        egui::Window::new(title).collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
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
        egui::Window::new(lang.t("Sitzungs-Vorlagen", "Session templates")).open(&mut open).default_width(480.0).show(ctx, |ui| {
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
                            if armed_button(ui, ("tpldel", i), "🗑", lang.t("Wirklich?", "Sure?")) {
                                actions.push(Action::TemplateDelete(i));
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
        egui::Window::new(lang.t("Sitzung mit Worktree schließen", "Close session with worktree"))
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
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
                    ui.colored_label(Color32::from_rgb(0xe5, 0xc0, 0x7b), format!("{} {}", w.changes, lang.t("Datei(en) mit nicht committeten Änderungen.", "file(s) with uncommitted changes.")));
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

/// Zeilen eines zweispaltigen Grids für die Claude-Startoptionen (#161): Modell, Effort, Permission-Mode, Agent.
/// Leere Auswahl = Standard von Claude Code (Flag wird nicht gesetzt). `false`, solange ein Name ungültig ist.
/// `bypassPermissions` nur mit `allow_bypass` (Dialog „Neue Sitzung“, gilt nur für diese Sitzung) und mit Warnung;
/// in den Standards gibt es es nicht.
pub(super) fn claude_opts_rows(ui: &mut Ui, lang: Lang, o: &mut ClaudeOpts, salt: &str, place: OptsPlace) -> bool {
    let allow_bypass = place != OptsPlace::Defaults;
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
        let c = if warn { Color32::from_rgb(0xe0, 0x6c, 0x75) } else { style::dim(ui) };
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
    let more_ok = claude_more_rows(ui, lang, o, salt, place);
    !bad_name(&o.model) && !bad_name(&o.agent) && more_ok
}

/// Zeilen „Ordner“ im Abschnitt „Weitere Claude-Optionen“: Text (ein Pfad je Zeile) bereinigt, ohne Doppelte.
fn dir_lines(s: &str) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for l in s.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if !v.iter().any(|x| x == l) {
            v.push(l.to_string());
        }
    }
    v
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
}

/// Ergebnis einer Pfadprüfung im Abschnitt, je Pfad zwischengespeichert (keine Dateisystem-Zugriffe in jedem Bild).
type PathCache = std::collections::HashMap<(String, bool), (Result<(), crate::claude_opts::PathIssue>, f64)>;

/// Wie lange ein Prüfergebnis gilt (Sekunden), solange der Abschnitt offen ist.
const PATH_CACHE_SECS: f64 = 3.0;

/// Abschnitt „Weitere Claude-Optionen“ (#182) im selben zweispaltigen Grid wie [`claude_opts_rows`]: Schalter als Häkchen
/// (auch `--safe-mode`, nur für den Nutzer) und die Wertoptionen, die nur der Nutzer setzen kann (Agenten über MCP nur
/// die Agenten-Schalter). Zugeklappt, solange nichts gesetzt ist. Warnung bei den Rechte erweiternden Optionen und bei
/// `--safe-mode`. `false`, solange ein Wert ungültig ist – auch zugeklappt. Pfade werden nur bei offenem Abschnitt im
/// Dateisystem geprüft (Ergebnis je Pfad zwischengespeichert); zugeklappt zählen nur Form und gespeicherte Ergebnisse.
fn claude_more_rows(ui: &mut Ui, lang: Lang, o: &mut ClaudeOpts, salt: &str, place: OptsPlace) -> bool {
    use crate::claude_opts::{agent_switch, check_path, valid_autocompact, valid_budget, valid_name, valid_path, valid_text, valid_tools, PathIssue, MAX_DIRS, MAX_NAME, MAX_PATH_LEN, MAX_TEXT_LEN, MAX_TOOLS_LEN, SWITCHES, USER_SWITCHES};
    let red = Color32::from_rgb(0xe0, 0x6c, 0x75);
    let amber = Color32::from_rgb(0xe5, 0xc0, 0x7b);
    let hint = |ui: &mut Ui, text: &str, c: Color32| {
        ui.label("");
        let r = hint_label(ui, text, c);
        #[cfg(test)]
        ui.data_mut(|d| d.insert_temp(Id::new(("dlg_mark", "more_hint", d.get_temp::<usize>(Id::new("dlg_hint_n")).unwrap_or(0))), r.rect));
        #[cfg(test)]
        ui.data_mut(|d| {
            let n = d.get_temp::<usize>(Id::new("dlg_hint_n")).unwrap_or(0);
            d.insert_temp(Id::new("dlg_hint_n"), n + 1);
        });
        let _ = r;
        ui.end_row();
    };

    let open_id = Id::new(("claude_more_open", salt));
    let mut open = ui.data(|d| d.get_temp::<bool>(open_id)).unwrap_or(!o.extra.is_empty());
    ui.label("");
    let arrow = if open { "▾" } else { "▸" };
    let title = format!("{arrow} {}", lang.t("Weitere Claude-Optionen", "More Claude options"));
    let set = o.extra.flags.len() + o.extra.value_names().len();
    let title = if set > 0 && !open { format!("{title} ({set})") } else { title };
    if ui.add(egui::Button::new(title).frame(false)).on_hover_text(lang.t("Schalter wie --chrome und weitere Optionen für Claude Code", "Switches such as --chrome and more options for Claude Code")).clicked() {
        open = !open;
    }
    ui.data_mut(|d| d.insert_temp(open_id, open));
    ui.end_row();
    #[cfg(test)]
    ui.data_mut(|d| d.insert_temp(Id::new(("claude_more_shown", salt)), open));

    // Pfade prüfen: Form immer; Dateisystem nur bei offenem Abschnitt und höchstens alle PATH_CACHE_SECS je Pfad.
    let cache_id = Id::new(("claude_more_paths", salt));
    let now = ui.input(|i| i.time);
    let mut cache: PathCache = ui.data(|d| d.get_temp(cache_id)).unwrap_or_default();
    let mut check = |p: &str, dir: bool| -> Result<(), PathIssue> {
        if !valid_path(p) {
            return Err(PathIssue::Invalid);
        }
        let key = (p.to_string(), dir);
        match cache.get(&key) {
            Some((r, t)) if !open || now - t < PATH_CACHE_SECS => *r,
            _ if !open => Ok(()),
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

    // Prüfen (auch zugeklappt): ungültige Werte sperren „Erstellen“/„Speichern“.
    let e = &o.extra;
    let dirs: Vec<String> = e.add_dirs.iter().map(|d| d.trim().to_string()).filter(|d| !d.is_empty()).collect();
    let dir_err = if dirs.len() > MAX_DIRS {
        Some(format!("{} {MAX_DIRS}", lang.t("Höchstens so viele Ordner:", "At most this many folders:")))
    } else {
        dirs.iter().find_map(|d| path_err(check(d, true), d, true))
    };
    let filled = |s: &str| !s.trim().is_empty();
    let tools_bad = |s: &str| filled(s) && !valid_tools(s.trim());
    let mcp = e.mcp_config.trim().to_string();
    let mcp_err = if mcp.is_empty() { None } else { path_err(check(&mcp, false), &mcp, false) };
    ui.data_mut(|d| d.insert_temp(cache_id, cache));
    let text_bad = filled(&e.append_system_prompt) && !valid_text(e.append_system_prompt.trim());
    let fallback_bad = filled(&e.fallback_model) && !valid_name(e.fallback_model.trim());
    let budget_bad = filled(&e.max_budget_usd) && !valid_budget(e.max_budget_usd.trim());
    let compact_bad = filled(&e.autocompact) && !valid_autocompact(e.autocompact.trim());
    let (allowed_bad, disallowed_bad) = (tools_bad(&e.allowed_tools), tools_bad(&e.disallowed_tools));
    let ok = dir_err.is_none() && mcp_err.is_none() && !allowed_bad && !disallowed_bad && !text_bad && !fallback_bad && !budget_bad && !compact_bad;
    if !open {
        if !ok {
            hint(ui, lang.t("Ein Wert unter „Weitere Claude-Optionen“ ist ungültig.", "A value under “More Claude options” is invalid."), red);
        }
        return ok;
    }

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
        hint(
            ui,
            match place {
                OptsPlace::Defaults => lang.t(
                    "⚠ --safe-mode schaltet Hooks und MCP-Server ab: Paneldeck erkennt dann weder Status noch Freigabe-Fragen dieser Sitzungen – auch der Schutz, dass Agenten keine Freigaben beantworten, greift dort nicht. Gilt nur für Sitzungen, die du selbst startest, nie für Sitzungen von Agenten.",
                    "⚠ --safe-mode turns off hooks and MCP servers: Paneldeck then sees neither the status nor the permission prompts of these sessions – the protection that keeps agents from answering permission prompts does not work there either. Applies only to sessions you start yourself, never to sessions created by agents.",
                ),
                _ => lang.t(
                    "⚠ --safe-mode schaltet Hooks und MCP-Server ab: Paneldeck erkennt dann weder Status noch Freigabe-Fragen dieser Sitzung – auch der Schutz, dass Agenten keine Freigaben beantworten, greift hier nicht.",
                    "⚠ --safe-mode turns off hooks and MCP servers: Paneldeck then sees neither the status nor the permission prompts of this session – the protection that keeps agents from answering permission prompts does not work here either.",
                ),
            },
            amber,
        );
    }

    ui.label(lang.t("Zusätzliche Ordner", "Extra folders")).on_hover_text("--add-dir");
    ui.horizontal(|ui| {
        // Eingabepuffer im Speicher von egui: leere Zeilen und Leerzeichen am Rand bleiben beim Tippen erhalten.
        let bid = Id::new(("claude_more_dirs", salt));
        let mut buf = ui.data(|d| d.get_temp::<String>(bid)).filter(|b| dir_lines(b) == e.add_dirs).unwrap_or_else(|| e.add_dirs.join("\n"));
        ui.add(egui::TextEdit::multiline(&mut buf).desired_rows(2).desired_width(260.0).hint_text(lang.t("ein Ordner je Zeile", "one folder per line")));
        if ui.button("…").on_hover_text(lang.t("Ordner hinzufügen", "Add folder")).clicked() {
            if let Some(p) = notify::pick_folder("") {
                if !buf.is_empty() && !buf.ends_with('\n') {
                    buf.push('\n');
                }
                buf.push_str(&p);
            }
        }
        e.add_dirs = dir_lines(&buf);
        ui.data_mut(|d| d.insert_temp(bid, buf));
    });
    ui.end_row();
    if let Some(err) = &dir_err {
        hint(ui, err, red);
    }

    ui.label(lang.t("Erlaubte Werkzeuge", "Allowed tools")).on_hover_text("--allowed-tools");
    ui.add(egui::TextEdit::singleline(&mut e.allowed_tools).hint_text("Bash(git log:*) Read").char_limit(MAX_TOOLS_LEN).desired_width(260.0));
    ui.end_row();
    let tools_err = lang.t("Nur Buchstaben, Ziffern, Leerzeichen und _ . , : * ( ) / @ - ~ = + [ ] ? # \\ (nicht am Anfang -, nicht am Ende \\).", "Only letters, digits, spaces and _ . , : * ( ) / @ - ~ = + [ ] ? # \\ (no leading -, no trailing \\).");
    if allowed_bad {
        hint(ui, tools_err, red);
    }

    ui.label(lang.t("Verbotene Werkzeuge", "Disallowed tools")).on_hover_text("--disallowed-tools");
    ui.add(egui::TextEdit::singleline(&mut e.disallowed_tools).hint_text("WebFetch Bash(curl:*)").char_limit(MAX_TOOLS_LEN).desired_width(260.0));
    ui.end_row();
    if disallowed_bad {
        hint(ui, tools_err, red);
    }

    ui.label(lang.t("MCP-Konfiguration", "MCP config")).on_hover_text("--mcp-config");
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut e.mcp_config).hint_text(lang.t("JSON-Datei (optional)", "JSON file (optional)")).char_limit(MAX_PATH_LEN).desired_width(230.0));
        if ui.button("…").on_hover_text(lang.t("Datei wählen", "Choose file")).clicked() {
            if let Some(p) = notify::pick_file(e.mcp_config.trim()) {
                e.mcp_config = p;
            }
        }
    });
    ui.end_row();
    if let Some(err) = &mcp_err {
        hint(ui, err, red);
    }

    ui.label(lang.t("System-Prompt ergänzen", "Append system prompt")).on_hover_text("--append-system-prompt");
    ui.add(egui::TextEdit::singleline(&mut e.append_system_prompt).hint_text(lang.t("Text (eine Zeile, optional)", "Text (one line, optional)")).char_limit(MAX_TEXT_LEN).desired_width(260.0));
    ui.end_row();
    if text_bad {
        hint(ui, lang.t("Eine Zeile ohne \" % ! ^ & | < > \\, nicht mit - beginnend.", "One line without \" % ! ^ & | < > \\, not starting with -."), red);
    }

    ui.label(lang.t("Ersatzmodell", "Fallback model")).on_hover_text("--fallback-model");
    ui.add(egui::TextEdit::singleline(&mut e.fallback_model).hint_text(lang.t("z. B. sonnet (optional)", "e.g. sonnet (optional)")).char_limit(MAX_NAME).desired_width(160.0));
    ui.end_row();
    if fallback_bad {
        hint(ui, lang.t("Nur Buchstaben, Ziffern und . _ : - (höchstens 100 Zeichen).", "Only letters, digits and . _ : - (at most 100 characters)."), red);
    }

    ui.label(lang.t("Budget (USD)", "Max budget (USD)")).on_hover_text("--max-budget-usd");
    ui.add(egui::TextEdit::singleline(&mut e.max_budget_usd).hint_text(lang.t("z. B. 5", "e.g. 5")).char_limit(12).desired_width(80.0));
    ui.end_row();
    if budget_bad {
        hint(ui, lang.t("Eine positive Zahl, z. B. 5 oder 2.50.", "A positive number, e.g. 5 or 2.50."), red);
    }

    ui.label(lang.t("Auto-Komprimierung", "Auto-compact")).on_hover_text("--autocompact");
    ui.add(egui::TextEdit::singleline(&mut e.autocompact).hint_text(lang.t("auto oder Tokens, z. B. 200k", "auto or tokens, e.g. 200k")).char_limit(12).desired_width(120.0));
    ui.end_row();
    if compact_bad {
        hint(ui, lang.t("„auto“ oder eine Tokenzahl, z. B. 200000 oder 200k.", "“auto” or a number of tokens, e.g. 200000 or 200k."), red);
    }

    if e.has_risky() {
        hint(
            ui,
            match place {
                OptsPlace::Defaults => lang.t(
                    "⚠ Zusätzliche Ordner, erlaubte Werkzeuge und eine MCP-Konfiguration erweitern, worauf Claude zugreifen und was es ohne Rückfrage tun darf. Als Standard gelten sie nur für Sitzungen, die du selbst startest – nie für Sitzungen von Agenten.",
                    "⚠ Extra folders, allowed tools and an MCP config widen what Claude may access and do without asking. As defaults they apply only to sessions you start yourself – never to sessions created by agents.",
                ),
                OptsPlace::NewSession => lang.t(
                    "⚠ Zusätzliche Ordner, erlaubte Werkzeuge und eine MCP-Konfiguration erweitern, worauf Claude in dieser Sitzung zugreifen und was es ohne Rückfrage tun darf.",
                    "⚠ Extra folders, allowed tools and an MCP config widen what Claude may access and do without asking in this session.",
                ),
                OptsPlace::Session => lang.t(
                    "⚠ Zusätzliche Ordner, erlaubte Werkzeuge und eine MCP-Konfiguration erweitern, worauf Claude in dieser Sitzung zugreifen und was es ohne Rückfrage tun darf – ab dem nächsten Start, auch wenn ein Agent die Sitzung angelegt hat.",
                    "⚠ Extra folders, allowed tools and an MCP config widen what Claude may access and do without asking in this session – from its next start, even if an agent created the session.",
                ),
            },
            amber,
        );
    }
    let dim = style::dim(ui);
    hint(
        ui,
        lang.t(
            "Die Werte setzt nur der Nutzer; Agenten können über MCP nur Schalter ohne Einfluss auf Rechte ergänzen. Ordner und Dateien werden bei jedem Start geprüft; fehlende werden mit Hinweis weggelassen.",
            "Only you set the values; agents can only add switches without effect on rights via MCP. Folders and files are checked at every start; missing ones are left out with a notice.",
        ),
        dim,
    );
    ok
}

/// Breite der Hinweistexte in den Dialogen (fester Umbruch, damit das Raster nicht von Bild zu Bild springt, #241).
pub(super) const HINT_W: f32 = 330.0;

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
    let mut out = NewDlgOut::Open;
    let mut opts_ok = true;
    // Esc schließt den Dialog nicht, solange eine Auswahlliste offen ist (die schließt Esc zuerst), und wird
    // verbraucht – kein Terminal dahinter bekommt es (#261, wie in den Einstellungen der Sitzung).
    let esc = !egui::Popup::is_any_open(ctx) && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
    let body_h = dialog_body_height(ctx);
    egui::Window::new(if d.claude { lang.t("Neue Claude-Sitzung", "New Claude session") } else { lang.t("Neue Sitzung", "New session") })
        .id(Id::new("new_session_dialog"))
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().id_salt("newdlg_scroll").max_height(body_h).min_scrolled_height(body_h).show(ui, |ui| {
                egui::Grid::new("newgrid").num_columns(2).spacing([8.0, 8.0]).show(ui, |ui| {
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
                    let cur = shells.iter().find(|s| s.id == d.shell_id).map(|s| s.label.clone()).unwrap_or_default();
                    egui::ComboBox::from_id_salt("shellsel").selected_text(cur).show_ui(ui, |ui| {
                        for s in shells {
                            ui.selectable_value(&mut d.shell_id, s.id.clone(), &s.label);
                        }
                    });
                    ui.end_row();
                    ui.label(lang.t("Ordner", "Folder"));
                    // WSL-Shell: Linux-Pfad mit Vervollständigung, Distribution wählbar (wechselt die Shell), #172.
                    d.picker.set_target(target);
                    let r = d.picker.show(ui, lang, wsl_recent);
                    if let Some(sid) = r.distro_changed.and_then(|dist| shells.iter().find(|s| s.id.strip_prefix("wsl:").is_some_and(|x| x.eq_ignore_ascii_case(&dist)))).map(|s| s.id.clone()) {
                        d.shell_id = sid;
                    }
                    d.cwd = d.picker.value();
                    ui.end_row();
                    ui.label(lang.t("Name (optional)", "Name (optional)"));
                    let te = ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(320.0));
                    if d.focus {
                        // Beim Öffnen: Tasten gehören dem Dialog, nicht dem Terminal dahinter (#255).
                        te.request_focus();
                        d.focus = false;
                    }
                    dlg_mark(ui, "name", &te);
                    ui.end_row();
                    ui.label(lang.t("Ordner im Baum", "Tree folder"));
                    ui.add(egui::TextEdit::singleline(&mut d.group).hint_text(lang.t("z. B. Backend/API (leer = keiner)", "e.g. Backend/API (empty = none)")).desired_width(320.0));
                    ui.end_row();
                    if claude_mode {
                        ui.label("");
                        ui.checkbox(&mut d.claude, lang.t("Claude Code starten", "Start Claude Code"));
                        ui.end_row();
                        if d.claude {
                            // Eine Auswahl statt zweier Häkchen (#241, #245): wie die Konversation beginnt und ob sie beim
                            // Neu laden/Neustart fortgesetzt wird.
                            ui.label(lang.t("Konversation", "Conversation"));
                            let r = session_settings::conv_combo(ui, "newdlg_conv", &mut d.conv, lang);
                            dlg_mark(ui, "conv", &r);
                            ui.end_row();
                            opts_ok = claude_opts_rows(ui, lang, &mut d.copts, "newdlg", OptsPlace::NewSession);
                            ui.label(lang.t("Start-Prompt", "Start prompt"));
                            ui.add(egui::TextEdit::singleline(&mut d.prompt).desired_width(320.0));
                            ui.end_row();
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
                    }
                });
            });
            ui.add_space(6.0);
            // Ungültiger oder fehlender WSL-Ordner: Erstellen gesperrt (Grund steht unter dem Ordnerfeld).
            let blocked = d.picker.blocking_error(lang);
            ui.horizontal(|ui| {
                let b = ui.add_enabled(opts_ok && blocked.is_none(), egui::Button::new(lang.t("Erstellen", "Create")));
                dlg_mark(ui, "create", &b);
                if b.clicked() {
                    out = NewDlgOut::Create;
                }
                if let Some(e) = &blocked {
                    b.on_disabled_hover_text(e.as_str());
                }
                let c = ui.button(lang.t("Abbrechen", "Cancel"));
                dlg_mark(ui, "cancel", &c);
                if c.clicked() {
                    out = NewDlgOut::Cancel;
                }
            });
        });
    if esc && out == NewDlgOut::Open {
        out = NewDlgOut::Cancel;
    }
    out
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
        let repo = std::env::temp_dir().join(format!("pd-dlg-repo-{}", std::process::id()));
        if !repo.join(".git").exists() {
            std::fs::create_dir_all(&repo).unwrap();
            crate::gitops::git(&repo, &["init", "-q"]).expect("git init");
        }
        let here = repo.to_string_lossy().to_string();
        let mut copts = ClaudeOpts { permission_mode: crate::claude_opts::BYPASS.into(), ..ClaudeOpts::default() };
        copts.extra.flags = vec!["chrome".into(), "ide".into(), "verbose".into(), "safe-mode".into()];
        copts.extra.add_dirs = (0..6).map(|i| format!("C:\\gibt-es-nicht\\ordner-{i}")).collect();
        copts.extra.allowed_tools = "Read".into();
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

    #[test]
    fn the_new_session_dialog_fits_small_screens_with_everything_open() {
        for size in [Vec2::new(1280.0, 720.0), Vec2::new(800.0, 600.0)] {
            let ctx = egui::Context::default();
            ctx.data_mut(|m| m.insert_temp(Id::new(("claude_more_open", "newdlg")), true));
            let mut d = big_dialog();
            for i in 0..6 {
                frame(&ctx, &mut d, size, i as f64 * 0.1, vec![]);
            }
            let screen = Rect::from_min_size(Pos2::ZERO, size);
            let win = ctx.memory(|m| m.area_rect(Id::new("new_session_dialog"))).expect("Fenster gezeichnet");
            assert!(screen.contains_rect(win), "{size:?}: Fenster {win:?} ragt aus dem Bildschirm");
            for k in ["create", "cancel"] {
                let r = mark(&ctx, k, 0).unwrap_or_else(|| panic!("{k} fehlt"));
                assert!(screen.contains_rect(r) && win.contains_rect(r), "{size:?}: {k} {r:?} nicht sichtbar");
            }
            assert!(d.repo.is_some(), "der Testordner liegt in einem Repository");
            // Die Warnung zu --safe-mode überdeckt die Schalter nicht
            let sw = mark(&ctx, "switches", 0).expect("Schalter gezeichnet");
            let hint = mark(&ctx, "more_hint", 0).expect("Warnung gezeichnet");
            assert!(!sw.intersects(hint.shrink(1.0)), "{size:?}: Warnung {hint:?} über den Schaltern {sw:?}");
            assert!(hint.width() <= HINT_W + 1.0, "fester Umbruch: {hint:?}");
        }
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
        assert_eq!(d.conv, ConvChoice::Resume, "Standard wie bisher: neu, beim Neu laden fortsetzen");
        frame(&ctx, &mut d, Vec2::new(1000.0, 700.0), 0.0, vec![]);
        assert!(mark(&ctx, "conv", 0).is_some(), "eine Auswahl im Dialog");
        assert_eq!((ConvChoice::Resume.continue_last(), ConvChoice::Resume.resume_later()), (false, true));
        assert_eq!((ConvChoice::ContinueLast.continue_last(), ConvChoice::ContinueLast.resume_later()), (true, true));
        assert_eq!((ConvChoice::AlwaysNew.continue_last(), ConvChoice::AlwaysNew.resume_later()), (false, false));
        for c in ConvChoice::ALL {
            assert_ne!(c.label(Lang::De), c.label(Lang::En));
            assert_ne!(c.tip(Lang::De), c.tip(Lang::En));
        }
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
                    ok = Some(claude_opts_rows(ui, Lang::En, o, "t", place));
                });
            });
        });
        ok.unwrap()
    }

    fn shown(ctx: &egui::Context) -> bool {
        ctx.data(|d| d.get_temp::<bool>(Id::new(("claude_more_shown", "t")))).unwrap_or(false)
    }

    fn extra(e: ExtraOpts) -> ClaudeOpts {
        ClaudeOpts { extra: e, ..Default::default() }
    }

    #[test]
    fn more_options_section() {
        // Ohne weitere Optionen zugeklappt; mit gesetzten Werten offen.
        let ctx = egui::Context::default();
        assert!(rows_ctx(&ctx, &mut ClaudeOpts::default(), OptsPlace::NewSession));
        assert!(!shown(&ctx), "zugeklappt, solange nichts gesetzt ist");
        let d = std::env::temp_dir().join(format!("pd-flags-ui-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("mcp.json");
        std::fs::write(&f, "{}").unwrap();
        let (dir, file) = (d.to_string_lossy().to_string(), f.to_string_lossy().to_string());
        let mut o = extra(ExtraOpts {
            flags: vec!["chrome".into(), "ide".into(), "safe-mode".into()],
            add_dirs: vec![format!("  {dir}  "), String::new(), dir.clone()],
            allowed_tools: "Bash(git log:*) Read".into(),
            disallowed_tools: "WebFetch".into(),
            mcp_config: file.clone(),
            append_system_prompt: "Be brief. ".into(),
            fallback_model: "sonnet".into(),
            max_budget_usd: "2.50".into(),
            autocompact: "200k".into(),
        });
        let ctx = egui::Context::default();
        assert!(rows_ctx(&ctx, &mut o, OptsPlace::NewSession), "gültige Werte");
        assert_eq!(o.extra.flags, ["chrome", "ide", "safe-mode"], "safe-mode ist ein Schalter für den Nutzer");
        assert!(shown(&ctx), "offen, sobald etwas gesetzt ist");
        assert_eq!(o.extra.add_dirs, [dir.clone()], "Ordnerzeilen bereinigt, ohne Doppelte");
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
            ExtraOpts { mcp_config: format!("{dir}{sep}missing.json"), ..Default::default() },
            ExtraOpts { mcp_config: dir.clone(), ..Default::default() },
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
        // Auch zugeklappt sperrt ein ungültiger Wert (Form) …
        let ctx = egui::Context::default();
        ctx.data_mut(|d| d.insert_temp(Id::new(("claude_more_open", "t")), false));
        assert!(!rows_ctx(&ctx, &mut extra(ExtraOpts { max_budget_usd: "x".into(), ..Default::default() }), OptsPlace::NewSession));
        assert!(!shown(&ctx));
        // … ohne Dateisystem-Zugriff: ein fehlender Ordner fällt zugeklappt nicht auf (geprüft wird beim Start) …
        let mut missing = extra(ExtraOpts { add_dirs: vec![format!("{dir}{sep}missing")], ..Default::default() });
        assert!(rows_ctx(&ctx, &mut missing, OptsPlace::NewSession), "zugeklappt: keine Prüfung im Dateisystem");
        // … aufgeklappt schon; das Ergebnis bleibt zwischengespeichert und gilt dann auch zugeklappt.
        ctx.data_mut(|d| d.insert_temp(Id::new(("claude_more_open", "t")), true));
        assert!(!rows_ctx(&ctx, &mut missing, OptsPlace::NewSession));
        ctx.data_mut(|d| d.insert_temp(Id::new(("claude_more_open", "t")), false));
        assert!(!rows_ctx(&ctx, &mut missing, OptsPlace::NewSession), "Ergebnis aus dem Zwischenspeicher");
        let _ = std::fs::remove_dir_all(d);
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
    }
}
