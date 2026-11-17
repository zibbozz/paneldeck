//! Einstellungen, Datei-Editor, Schrift.

use super::*;

impl App {
    pub(super) fn settings_ui(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            // Eine nicht abgeschlossene Kürzel-Aufnahme würde sonst alle Tastenkürzel dauerhaft sperren.
            self.capture_key = None;
            if self.remember_off_ask == Some(self.wins.cur) {
                self.remember_off_ask = None; // Einstellungen geschlossen: Rückfrage gilt als abgebrochen
            }
            return;
        }
        self.remember_off_ui(ctx);
        let lang = self.settings.language;
        let mut open = true;
        egui::Window::new(lang.t("Einstellungen", "Settings"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .min_width(560.0_f32.min(ctx.content_rect().width() - 40.0))
            .max_width(560.0_f32.min(ctx.content_rect().width() - 40.0))
            .max_height((ctx.content_rect().height() - 60.0).max(200.0))
            .show(ctx, |ui| {
                let tabs = [
                    lang.t("Allgemein", "General"),
                    lang.t("Darstellung", "Appearance"),
                    "Claude",
                    lang.t("Benachrichtigungen", "Notifications"),
                    lang.t("Fenster", "Window"),
                    lang.t("Tastenkürzel", "Shortcuts"),
                    lang.t("Profile", "Profiles"),
                    lang.t("Agenten", "Agents"),
                    lang.t("Daten & Sicherung", "Data & backup"),
                    lang.t("Info", "About"),
                    // Index 10 (`models_ui::SETTINGS_TAB`): am Ende, damit die festen Indizes oben gültig bleiben.
                    lang.t("Modelle", "Models"),
                    // Index 11 (`palette_ui::SETTINGS_TAB`), angezeigt hinter „Darstellung“.
                    lang.t("Farben", "Colors"),
                ];
                // Anzeigereihenfolge (die Indizes bleiben fest).
                const ORDER: [usize; 12] = [0, 1, palette_ui::SETTINGS_TAB, 2, 3, 4, 5, 6, 7, 8, 9, models_ui::SETTINGS_TAB];
                ui.horizontal_wrapped(|ui| {
                    for i in ORDER {
                        let t = tabs[i];
                        let r = style::tab(ui, t, self.settings_tab == i);
                        if r.clicked() {
                            self.settings_tab = i;
                        }
                    }
                });
                ui.add_space(S2);
                ui.separator();
                ui.add_space(S2);

                // Zeile: Beschriftung (+ Erklärung) links, Steuerelement rechts
                fn row(ui: &mut Ui, label: &str, hint: &str, add: impl FnOnce(&mut Ui)) {
                    let w = ui.available_width();
                    ui.allocate_ui_with_layout(Vec2::new(w, 42.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        let x0 = ui.cursor().min.x;
                        ui.allocate_ui_with_layout(
                            Vec2::new(240.0, 42.0),
                            egui::Layout::top_down(egui::Align::LEFT).with_main_align(egui::Align::Center),
                            |ui| {
                                ui.label(label);
                                if !hint.is_empty() {
                                    ui.label(RichText::new(hint).small().color(style::dim(ui)));
                                }
                            },
                        );
                        let used = ui.cursor().min.x - x0;
                        ui.add_space((250.0 - used).max(0.0));
                        add(ui);
                    });
                    ui.add_space(4.0);
                }

                egui::ScrollArea::vertical().max_height((ctx.content_rect().height() - 190.0).clamp(160.0, 440.0)).auto_shrink([false, true]).show(ui, |ui| {
                match self.settings_tab {
                    0 => {
                        row(ui, lang.t("Sprache", "Language"), "", |ui| {
                            let a = ui.selectable_value(&mut self.settings.language, Lang::De, "Deutsch");
                            let b = ui.selectable_value(&mut self.settings.language, Lang::En, "English");
                            // Eine eigene Wahl beendet die Überschreibung durch `--lang` (sie wird dann gespeichert).
                            if a.clicked() || b.clicked() {
                                state::clear_lang_override();
                            }
                        });
                        let cur = self.shells.iter().find(|s| s.id == self.settings.default_shell).map(|s| s.label.clone()).unwrap_or_default();
                        row(ui, lang.t("Standard-Shell", "Default shell"), lang.t("Für neue Sitzungen und Teilungen", "For new sessions and splits"), |ui| {
                            egui::ComboBox::from_id_salt("defshell").selected_text(cur).show_ui(ui, |ui| {
                                for s in &self.shells {
                                    ui.selectable_value(&mut self.settings.default_shell, s.id.clone(), &s.label);
                                }
                            });
                        });
                        row(ui, lang.t("Shell-Integration", "Shell integration"), lang.t("Befehlsmarken, Ordner-Tracking (wirkt für neue Sitzungen; ändert keine Profile)", "Command marks, folder tracking (applies to new sessions; does not change profiles)"), |ui| {
                            style::toggle(ui, &mut self.settings.shell_integration);
                        });
                        // #175: kurzer Hinweis in der Zeile, die Abwägung ausführlich im Tooltip des Schalters.
                        row(
                            ui,
                            lang.t("Umgebung von Paneldeck erben", "Inherit Paneldeck's environment"),
                            lang.t("Aus: frisch aus Windows (setx wirkt sofort). An: die Umgebung beim Start von Paneldeck.", "Off: fresh from Windows (setx applies at once). On: the environment Paneldeck was started with."),
                            |ui| {
                                style::toggle(ui, &mut self.settings.inherit_env).on_hover_text(lang.t(
                                    "Aus (empfohlen): Neue Terminals lesen die Umgebungsvariablen frisch aus Windows – mit setx oder in den Systemeinstellungen geänderte Variablen gelten sofort, ohne Paneldeck neu zu starten; eine eigene Umgebung der Shell, aus der Paneldeck gestartet wurde, geht dabei für Variablen, die auch in Windows gesetzt sind (z. B. PATH), verloren.\n\nAn: Neue Terminals bekommen genau die Umgebung, mit der Paneldeck gestartet wurde (z. B. aus einer Shell mit eigenem PATH oder eigenen Variablen); in Windows später geänderte Variablen gelten dann erst nach einem Neustart von Paneldeck.\n\nIn beiden Fällen gelten die Variablen des Profils und Paneldecks eigene Variablen. Wirkt für neue Terminals.",
                                    "Off (recommended): new terminals read the environment variables fresh from Windows – variables changed with setx or in the system settings apply at once, without restarting Paneldeck; a custom environment of the shell Paneldeck was started from is lost for variables that Windows also sets (e.g. PATH).\n\nOn: new terminals get exactly the environment Paneldeck was started with (e.g. from a shell with its own PATH or variables); variables changed in Windows later only apply after restarting Paneldeck.\n\nProfile variables and Paneldeck's own variables apply either way. Applies to new terminals.",
                                ));
                            },
                        );
                        row(ui, lang.t("Befehl fertig melden", "Notify when command finishes"), lang.t("ab dieser Dauer in Sekunden (0 = aus)", "after this many seconds (0 = off)"), |ui| {
                            ui.add(egui::DragValue::new(&mut self.settings.cmd_notify_secs).range(0..=3600).speed(1.0).suffix(" s"));
                        });
                        row(ui, lang.t("Schließen bestätigen", "Confirm close"), lang.t("Bei laufenden Sitzungen nachfragen", "Ask for running sessions"), |ui| {
                            style::toggle(ui, &mut self.settings.confirm_close);
                        });
                        row(
                            ui,
                            lang.t("Sitzungen merken", "Remember sessions"),
                            lang.t(
                                "Terminals, Chats, Projekte und Ordner beim nächsten Start wiederherstellen. Aus: jeder Start ist leer, Chatverläufe werden beim Beenden gelöscht.",
                                "Restore terminals, chats, projects and folders on the next start. Off: every start is empty, chat histories are deleted on exit.",
                            ),
                            |ui| {
                                // Ausschalten erst nach Rückfrage (verwirft beim nächsten Beenden Projekte und Ordner).
                                let mut on = self.settings.remember_sessions;
                                style::toggle(ui, &mut on);
                                if on && !self.settings.remember_sessions {
                                    self.settings.remember_sessions = true;
                                } else if !on && self.settings.remember_sessions {
                                    self.remember_off_ask = Some(self.wins.cur);
                                }
                            },
                        );
                        ui.add_space(S2);
                        row(ui, lang.t("Diagnose", "Diagnostics"), lang.t("Systeminfos für Fehlerberichte", "System info for bug reports"), |ui| {
                            if ui.button(lang.t("In Zwischenablage kopieren", "Copy to clipboard")).clicked() {
                                ctx.copy_text(self.diagnostics());
                            }
                            #[cfg(windows)]
                            if ui.button(lang.t("Datenordner öffnen", "Open data folder")).clicked() {
                                let _ = std::process::Command::new("explorer").arg(&self.dir).spawn();
                            }
                        });
                        ui.label(RichText::new(format!("{}: {}", lang.t("Datenordner", "Data folder"), self.dir.display())).small().color(style::dim(ui)));
                        ui.add_space(S1);
                        ui.label(RichText::new(lang.t(
                            "Tastenkürzel findest und änderst du im Tab „Tastenkürzel“. Strg+Umschalt+P öffnet die Befehls-Palette.",
                            "Find and change keyboard shortcuts in the “Shortcuts” tab. Ctrl+Shift+P opens the command palette.",
                        )).small().color(style::dim(ui)));
                    }
                    1 => {
                        row(ui, "Theme", "", |ui| {
                            egui::ComboBox::from_id_salt("themesel").selected_text(self.settings.theme.clone()).show_ui(ui, |ui| {
                                for t in &self.themes {
                                    ui.selectable_value(&mut self.settings.theme, t.name.clone(), t.name.as_str());
                                }
                            });
                        });
                        row(ui, lang.t("Schriftgröße", "Font size"), lang.t("Terminalschrift (Cascadia Mono)", "Terminal font (Cascadia Mono)"), |ui| {
                            ui.add(egui::Slider::new(&mut self.settings.font_size, 9.0..=28.0).suffix(" pt"));
                        });
                        row(ui, lang.t("UI-Skalierung", "UI scale"), lang.t("Gesamte Oberfläche vergrößern/verkleinern", "Scale the whole interface"), |ui| {
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
                        row(ui, lang.t("Deckkraft", "Opacity"), lang.t("Terminal und Seitenleiste", "Terminal and sidebar"), |ui| {
                            ui.add(egui::Slider::new(&mut self.settings.opacity, 0.3..=1.0).custom_formatter(|v, _| format!("{:.0} %", v * 100.0)));
                        });
                        row(ui, lang.t("Animationen", "Animations"), lang.t("Weiche Übergänge bei Hover, Fokus und Panels", "Smooth transitions for hover, focus and panels"), |ui| {
                            style::toggle(ui, &mut self.settings.animations);
                        });
                        row(
                            ui,
                            lang.t("Terminalschrift", "Terminal font"),
                            lang.t(
                                "Jede installierte Festbreitenschrift, z. B. Nerd Fonts. Powerline-, Rahmen- und Blockzeichen zeichnet Paneldeck selbst, damit sie lückenlos anschließen.",
                                "Any installed monospace font, e.g. Nerd Fonts. Paneldeck draws Powerline, box-drawing and block characters itself so they line up without gaps.",
                            ),
                            |ui| {
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
                            },
                        );
                        row(
                            ui,
                            lang.t("Blinkender Cursor", "Blinking cursor"),
                            lang.t("Wenn das Programm es anfordert; nur im fokussierten Terminal", "When the program asks for it; only in the focused terminal"),
                            |ui| {
                                style::toggle(ui, &mut self.settings.cursor_blink);
                            },
                        );
                        row(ui, lang.t("Eigene Themes", "Custom themes"), lang.t("JSON-Dateien im Ordner themes (Beispiel liegt dabei)", "JSON files in the themes folder (example included)"), |ui| {
                            if ui.button(lang.t("Neu laden", "Reload")).clicked() {
                                self.reload_themes();
                            }
                            #[cfg(windows)]
                            if ui.button(lang.t("Ordner öffnen", "Open folder")).clicked() {
                                let _ = std::process::Command::new("explorer").arg(self.themes_dir()).spawn();
                            }
                        });
                        row(ui, "Scrollback", lang.t("Zeilen pro Terminal (wirkt für neue Sitzungen)", "Lines per terminal (applies to new sessions)"), |ui| {
                            ui.add(egui::DragValue::new(&mut self.settings.scrollback_lines).range(1000..=100_000).speed(100.0));
                        });
                        row(
                            ui,
                            lang.t("Mausrad in Vollbildprogrammen", "Mouse wheel in full-screen programs"),
                            lang.t(
                                "Für Programme wie less oder man ohne eigene Mausunterstützung. Umschalt+Mausrad scrollt den Verlauf von Paneldeck (Vollbildprogramme haben keinen).",
                                "For programs like less or man without their own mouse support. Shift+wheel scrolls Paneldeck's scrollback (full-screen programs have none).",
                            ),
                            |ui| {
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
                            },
                        );
                        #[cfg(windows)]
                        {
                            row(
                                ui,
                                lang.t("Mitgelieferten Konsolen-Host verwenden (empfohlen)", "Use the bundled console host (recommended)"),
                                lang.t(
                                    "Neuere ConPTY (OpenConsole) neben paneldeck.exe: reicht Farbabfragen und flimmerfreie Ausgabe durch. Aus: Konsolen-Host von Windows. Wirkt für neue Sitzungen.",
                                    "Newer ConPTY (OpenConsole) next to paneldeck.exe: passes colour queries and flicker-free output through. Off: the console host built into Windows. Applies to new sessions.",
                                ),
                                |ui| {
                                    style::toggle(ui, &mut self.settings.bundled_conpty);
                                },
                            );
                            if self.settings.bundled_conpty && crate::conpty::bundled_dll().is_none() {
                                ui.label(RichText::new(lang.t(
                                    "conpty.dll und OpenConsole.exe fehlen neben paneldeck.exe – Sitzungen nutzen den Konsolen-Host von Windows.",
                                    "conpty.dll and OpenConsole.exe are missing next to paneldeck.exe – sessions use the console host built into Windows.",
                                )).small().color(Color32::from_rgb(0xe5, 0xc0, 0x7b)));
                                ui.add_space(4.0);
                            }
                        }
                        row(
                            ui,
                            lang.t("Programme dürfen in die Zwischenablage kopieren (OSC 52)", "Allow programs to copy to the clipboard (OSC 52)"),
                            lang.t(
                                "Z. B. Claude Code, vim oder tmux in WSL- und SSH-Sitzungen. Nur die fokussierte Sitzung, nur Schreiben (nie Lesen), höchstens 1 MB. Achtung: Auch ein Agent mit Steuer-Zugriff (MCP) kann so Text in deine Zwischenablage legen.",
                                "E.g. Claude Code, vim or tmux in WSL and SSH sessions. Only the focused session, write only (never read), at most 1 MB. Note: an agent with control access (MCP) can also put text on your clipboard this way.",
                            ),
                            |ui| {
                                style::toggle(ui, &mut self.settings.osc52_copy);
                            },
                        );
                    }
                    2 => {
                        row(ui, lang.t("Claude-Modus", "Claude mode"), lang.t("Zeigt Claude-Funktionen: Start, Status, Kosten, Verlauf", "Shows Claude features: start, status, cost, history"), |ui| {
                            style::toggle(ui, &mut self.settings.claude_mode);
                        });
                        if self.settings.claude_mode {
                            row(ui, lang.t("Fortsetzen", "Resume"), lang.t("Claude-Sitzungen beim Start mit --resume wiederherstellen", "Restore Claude sessions on start with --resume"), |ui| {
                                style::toggle(ui, &mut self.settings.claude_resume);
                            });
                            ui.add_space(S2);
                            ui.label(lang.t("Standards für neue Claude-Sitzungen", "Defaults for new Claude sessions"));
                            ui.label(RichText::new(lang.t(
                                "Gelten für Sitzungen aus dem Dialog „Neue Sitzung“ (Vorauswahl), dem Kontextmenü, Vorlagen und über MCP, wenn nichts angegeben ist. „Standard von Claude Code“ setzt keine Option. bypassPermissions lässt sich nur im Dialog „Neue Sitzung“ für eine einzelne Sitzung wählen. Ohne Permission-Mode starten Agenten-Sitzungen mit „default“.",
                                "Apply to sessions from the “New session” dialog (preselection), the context menu, templates and via MCP when nothing is given. “Claude Code default” sets no option. bypassPermissions can only be chosen in the “New session” dialog for a single session. Without a permission mode, agent sessions start with “default”.",
                            )).small().color(style::dim(ui)));
                            ui.add_space(S1);
                            if self.settings.claude_bypass_dropped {
                                ui.label(RichText::new(lang.t(
                                    "Hinweis: bypassPermissions stand als Standard in settings.json und wurde entfernt – es ist nur noch im Dialog „Neue Sitzung“ für eine einzelne Sitzung wählbar.",
                                    "Note: bypassPermissions was set as a default in settings.json and has been removed – it can now only be chosen in the “New session” dialog for a single session.",
                                )).small().color(Color32::from_rgb(0xe5, 0xc0, 0x7b)));
                                ui.add_space(S1);
                            }
                            let before = self.settings.claude_defaults.clone();
                            egui::Grid::new("claude_defaults").num_columns(2).spacing([8.0, 8.0]).show(ui, |ui| {
                                // Ungültige Namen bleiben nur im Eingabefeld; gespeichert und angewendet werden nur gültige Werte.
                                super::dialogs::claude_opts_rows(ui, lang, &mut self.settings.claude_defaults, "defaults", false);
                            });
                            if self.settings.claude_defaults != before {
                                // Neue Standards gelten auch im Dialog wieder (statt der zuletzt dort gewählten Optionen).
                                self.settings.last_claude_opts = None;
                            }
                            ui.add_space(S2);
                            row(ui, lang.t("Tagesbudget", "Daily budget"), lang.t("Hinweis, wenn die geschätzten Tageskosten (UTC-Tag, alle Sitzungen) den Betrag in USD überschreiten; 0 = aus", "Notice when estimated daily cost (UTC day, all sessions) exceeds this USD amount; 0 = off"), |ui| {
                                ui.add(egui::DragValue::new(&mut self.settings.daily_budget).range(0.0..=100000.0).speed(0.5).prefix("$ ").fixed_decimals(2));
                            });
                            row(ui, lang.t("Sitzungsbudget", "Session budget"), lang.t("Hinweis, sobald eine einzelne Sitzung die geschätzten Kosten in USD überschreitet (einmal je Sitzung); 0 = aus", "Notice once a single session exceeds this estimated USD cost; 0 = off"), |ui| {
                                ui.add(egui::DragValue::new(&mut self.settings.session_budget).range(0.0..=100000.0).speed(0.5).prefix("$ ").fixed_decimals(2));
                            });
                            ui.add_space(S2);
                            ui.label(lang.t("Preise für die Kostenschätzung (USD pro Million Token)", "Prices for the cost estimate (USD per million tokens)"));
                            ui.label(RichText::new(lang.t("Nur eine Schätzung auf Basis der lokalen Transkripte. Unbekannte Modelle werden wie Sonnet gerechnet.", "Estimate only, based on local transcripts. Unknown models are priced like Sonnet.")).small().color(style::dim(ui)));
                            ui.add_space(S1);
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
                            if ui.button(lang.t("Auf Standardwerte zurücksetzen", "Reset to defaults")).clicked() {
                                self.settings.prices = Default::default();
                            }
                            ui.add_space(S2);
                            ui.label("Hooks");
                            ui.label(RichText::new(lang.t(
                                "Hub-Sitzungen nutzen Hooks automatisch (per --settings). Global eintragen, damit auch manuell gestartetes claude gemeldet wird.",
                                "Hub sessions use hooks automatically (via --settings). Install globally to also track manually started claude.",
                            )).small().color(style::dim(ui)));
                            ui.add_space(S1);
                            ui.horizontal(|ui| {
                                if self.hooks_global {
                                    ui.colored_label(Color32::from_rgb(0x4c, 0xc2, 0x6a), lang.t("global installiert", "installed globally"));
                                    if ui.button(lang.t("Entfernen", "Remove")).clicked() {
                                        self.hooks_msg = match hooks::global_settings_path().map(|p| hooks::uninstall_global(&p)) {
                                            Some(Ok(())) => String::new(),
                                            Some(Err(e)) => e,
                                            None => state::t("~/.claude nicht gefunden", "~/.claude not found").into(),
                                        };
                                        self.hooks_global = hooks::global_settings_path().is_some_and(|p| hooks::global_installed(&p));
                                    }
                                } else if ui.button(lang.t("In ~/.claude/settings.json installieren", "Install into ~/.claude/settings.json")).clicked() {
                                    self.hooks_msg = match hooks::global_settings_path().map(|p| hooks::install_global(&p, &self.exe)) {
                                        Some(Ok(())) => String::new(),
                                        Some(Err(e)) => e,
                                        None => state::t("~/.claude nicht gefunden", "~/.claude not found").into(),
                                    };
                                    self.hooks_global = hooks::global_settings_path().is_some_and(|p| hooks::global_installed(&p));
                                }
                            });
                            if !self.hooks_msg.is_empty() {
                                ui.colored_label(Color32::LIGHT_RED, &self.hooks_msg);
                            }
                        }
                    }
                    3 => {
                        row(ui, "Windows-Toast", lang.t("Bei Freigabe oder Fertig, wenn das Fenster im Hintergrund ist", "On approval or done while the window is in the background"), |ui| {
                            style::toggle(ui, &mut self.settings.notify_toast);
                        });
                        row(ui, lang.t("Ton", "Sound"), lang.t("Mit dem Toast", "With the toast"), |ui| {
                            style::toggle(ui, &mut self.settings.notify_sound);
                        });
                        row(ui, lang.t("Taskleiste blinken", "Flash taskbar"), "", |ui| {
                            style::toggle(ui, &mut self.settings.notify_flash);
                        });
                        row(
                            ui,
                            lang.t("Benachrichtigungen von Programmen", "Notifications from programs"),
                            lang.t(
                                "OSC 9 und OSC 777 – nur für Sitzungen, die du gerade nicht ansiehst, höchstens einige pro Minute",
                                "OSC 9 and OSC 777 – only for sessions you are not looking at, at most a few per minute",
                            ),
                            |ui| {
                                style::toggle(ui, &mut self.settings.program_notify);
                            },
                        );
                        row(
                            ui,
                            lang.t("Toast bei Glocke", "Toast on bell"),
                            lang.t(
                                "Die Glocke (BEL) einer Sitzung, die du gerade nicht ansiehst, markiert sie immer in der Seitenleiste; an: zusätzlich Toast bzw. Hinweis",
                                "A bell (BEL) in a session you are not looking at always marks it in the sidebar; on: also a toast or notice",
                            ),
                            |ui| {
                                style::toggle(ui, &mut self.settings.bell_toast);
                            },
                        );
                        row(
                            ui,
                            lang.t("Fortschritt anzeigen", "Show progress"),
                            lang.t("Fortschritt von Programmen (OSC 9;4) in der Seitenleiste und in der Taskleiste", "Progress reported by programs (OSC 9;4) in the sidebar and on the taskbar"),
                            |ui| {
                                style::toggle(ui, &mut self.settings.progress_indicator);
                            },
                        );
                    }
                    6 => {
                        let shells: Vec<(String, String)> = self.shells.iter().map(|s| (s.id.clone(), s.label.clone())).collect();
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
                        } else {
                            ui.horizontal_wrapped(|ui| {
                                for (i, p) in self.settings.profiles.iter().enumerate() {
                                    if ui.selectable_label(self.profile_sel == i, &p.name).clicked() {
                                        self.profile_sel = i;
                                    }
                                }
                            });
                            ui.separator();
                            let sel = self.profile_sel.min(self.settings.profiles.len() - 1);
                            let p = &mut self.settings.profiles[sel];
                            row(ui, lang.t("Name", "Name"), "", |ui| {
                                ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(240.0));
                            });
                            row(ui, "Shell", lang.t("Leer = Standard-Shell", "Empty = default shell"), |ui| {
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
                            row(ui, lang.t("Eigenes Programm", "Custom program"), lang.t("Überschreibt die Shell, z. B. ssh.exe oder wsl.exe", "Overrides the shell, e.g. ssh.exe or wsl.exe"), |ui| {
                                ui.add(egui::TextEdit::singleline(&mut p.program).desired_width(240.0));
                            });
                            row(ui, lang.t("Argumente", "Arguments"), "", |ui| {
                                ui.add(egui::TextEdit::singleline(&mut p.args).desired_width(240.0));
                            });
                            row(ui, lang.t("Startordner", "Start folder"), "", |ui| {
                                ui.add(egui::TextEdit::singleline(&mut p.cwd).desired_width(240.0));
                            });
                            row(ui, lang.t("Startbefehl", "Startup command"), lang.t("Wird nach dem Start eingegeben (vor `claude`)", "Typed after start (before `claude`)"), |ui| {
                                ui.add(egui::TextEdit::singleline(&mut p.startup).desired_width(240.0));
                            });
                            ui.label(lang.t("Umgebungsvariablen (NAME=WERT, eine pro Zeile)", "Environment variables (NAME=VALUE, one per line)"));
                            ui.add(egui::TextEdit::multiline(&mut p.env).desired_rows(3).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace));
                            ui.add_space(S1);
                            row(ui, "Theme", "", |ui| {
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
                            row(ui, lang.t("Schriftgröße", "Font size"), lang.t("0 = global", "0 = global"), |ui| {
                                ui.add(egui::DragValue::new(&mut p.font_size).range(0.0..=40.0).speed(0.2));
                            });
                            row(ui, lang.t("Farbe", "Color"), lang.t("Farbmarke der Sitzung", "Session color mark"), |ui| {
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
                            row(ui, lang.t("Als Administrator", "As administrator"), lang.t("Öffnet ein eigenes Fenster (UAC); nicht eingebettet", "Opens its own window (UAC); not embedded"), |ui| {
                                style::toggle(ui, &mut p.admin);
                            });
                        }
                    }
                    5 => {
                        let de = lang == Lang::De;
                        ui.label(RichText::new(lang.t("Klicke auf ein Kürzel und drücke die neue Tastenkombination (Esc bricht ab).", "Click a shortcut and press the new key combination (Esc cancels).")).small().color(style::dim(ui)));
                        ui.add_space(S1);
                        let current: Vec<(&str, String)> = keys::DEFAULTS.iter().map(|(id, d)| (*id, self.settings.keys.get(*id).cloned().unwrap_or_else(|| d.to_string()))).collect();
                        for (id, cur) in &current {
                            // Leer = kein Kürzel (z. B. „Alle Sitzungen schließen“): nie „doppelt“.
                            let dup = !cur.is_empty() && current.iter().filter(|(_, c)| c == cur).count() > 1;
                            let capturing = self.capture_key.as_deref() == Some(*id);
                            row(ui, keys::label(id, de), if dup { lang.t("Doppelt belegt!", "Duplicate!") } else { "" }, |ui| {
                                let txt = if capturing {
                                    lang.t("Taste drücken …", "Press keys …").to_string()
                                } else if cur.is_empty() {
                                    lang.t("(keins)", "(none)").to_string()
                                } else {
                                    keys::pretty(cur, de)
                                };
                                let b = egui::Button::new(RichText::new(txt).monospace()).min_size(Vec2::new(190.0, 24.0));
                                let b = if capturing { b.fill(style::ACCENT.gamma_multiply(0.35)) } else { b };
                                if ui.add(b).clicked() {
                                    self.capture_key = Some(id.to_string());
                                }
                                if self.settings.keys.contains_key(*id) && ui.small_button(lang.t("Zurücksetzen", "Reset")).clicked() {
                                    self.settings.keys.remove(*id);
                                }
                            });
                        }
                        ui.label(RichText::new(lang.t(
                            "Fest: Strg+Umschalt+1…9 Projekt · Alt+1…9 Sitzung · Strg+Alt+Pfeile Bereich",
                            "Fixed: Ctrl+Shift+1…9 project · Alt+1…9 session · Ctrl+Alt+arrows pane",
                        )).small().color(style::dim(ui)));
                        // Aufnahme
                        if let Some(id) = self.capture_key.clone() {
                            let ev = ctx.input(|i| i.events.clone());
                            for e in ev {
                                if let Event::Key { key, pressed: true, modifiers, .. } = e {
                                    if key == Key::Escape {
                                        self.capture_key = None;
                                    } else {
                                        self.settings.keys.insert(id.clone(), keys::format(modifiers, key));
                                        self.capture_key = None;
                                    }
                                }
                            }
                        }
                    }
                    7 => self.mcp_settings_ui(ui, ctx),
                    models_ui::SETTINGS_TAB => self.models_settings_ui(ui, ctx),
                    palette_ui::SETTINGS_TAB => palette_ui::settings_view(ui, &mut self.settings.palette, lang),
                    8 => self.data_settings_ui(ui),
                    9 => {
                        ui.label(RichText::new(format!("Paneldeck {}", env!("CARGO_PKG_VERSION"))).heading());
                        ui.label(RichText::new(lang.t("Terminal-Zentrale für Claude Code und viele Terminals – funktioniert offline; kontaktiert GitHub nur, wenn du nach Updates suchst oder die automatische Prüfung einschaltest.", "Terminal hub for Claude Code and many terminals – works offline; only contacts GitHub when you check for updates or switch on the automatic check.")).color(style::dim(ui)));
                        ui.add_space(S2);
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
                        ui.add_space(S2);
                        ui.horizontal(|ui| {
                            if ui.button(lang.t("Diagnose kopieren", "Copy diagnostics")).clicked() {
                                ctx.copy_text(self.diagnostics());
                            }
                            if ui.button(lang.t("Fehlerbericht kopieren", "Copy error report")).on_hover_text(lang.t("Diagnose und Ende des Absturzprotokolls in einem Schritt", "Diagnostics plus the end of the crash log in one step")).clicked() {
                                ctx.copy_text(self.error_report());
                            }
                            if ui.button(if self.show_licenses { lang.t("Lizenzen ausblenden", "Hide licenses") } else { lang.t("Lizenzen anzeigen", "Show licenses") }).clicked() {
                                self.show_licenses = !self.show_licenses;
                            }
                        });
                        self.update_about_ui(ui); // Abschnitt „Updates“ (update/ui.rs)
                        if self.show_licenses {
                            ui.add_space(S1);
                            egui::ScrollArea::vertical().max_height(220.0).id_salt("lic").show(ui, |ui| {
                                ui.label(RichText::new(include_str!("../../THIRD_PARTY.md")).small().monospace());
                            });
                        }
                    }
                    _ => {
                        if crate::cli::get().no_tray {
                            ui.label(RichText::new(lang.t("Gestartet mit --no-tray: kein Tray-Symbol, „Schließen → Tray“ und „Minimieren → Tray“ wirken bei diesem Start nicht.", "Started with --no-tray: no tray icon, “Close → tray” and “Minimize → tray” have no effect for this start.")).small().color(style::dim(ui)));
                        }
                        row(ui, lang.t("Schließen → Tray", "Close → tray"), lang.t("Das Fenster verschwindet im Tray (Beenden über das Tray-Menü)", "The window goes to the tray (quit via the tray menu)"), |ui| {
                            style::toggle(ui, &mut self.settings.close_to_tray);
                        });
                        row(ui, lang.t("Minimieren → Tray", "Minimize → tray"), lang.t("Kein Taskleistenbutton mehr; Klick aufs Tray-Symbol holt das Fenster zurück", "No taskbar button; clicking the tray icon brings the window back"), |ui| {
                            style::toggle(ui, &mut self.settings.minimize_to_tray);
                        });
                        ui.add_space(S2);
                        ui.label(RichText::new(lang.t("Windows-Integration (nur für dich, ohne Administratorrechte)", "Windows integration (current user only, no admin rights)")).strong());
                        if ui.small_button(lang.t("Status aktualisieren", "Refresh status")).clicked() || (!self.win_probed) {
                            self.win_autostart = crate::integration::autostart_status();
                            self.win_context = crate::integration::context_status();
                            self.win_probed = true;
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
                                ui.label(RichText::new(lang.t("→ anderes Paneldeck", "→ another Paneldeck")).small().color(Color32::from_rgb(0xe5, 0xc0, 0x7b))).on_hover_text(text);
                            }
                        }
                        row(ui, lang.t("Mit Windows starten", "Start with Windows"), lang.t("Startet minimiert im Tray", "Starts minimized in the tray"), |ui| {
                            let mut v = self.win_autostart.is_mine();
                            if style::toggle(ui, &mut v).changed() {
                                match crate::integration::set_autostart(v, &exe) {
                                    Ok(()) => self.win_autostart = if v { crate::integration::Status::Mine } else { crate::integration::Status::Off },
                                    Err(e) => err = Some(e),
                                }
                            }
                            other_hint(ui, lang, &self.win_autostart, lang.t("Der Autostart-Eintrag", "The autostart entry"));
                        });
                        row(ui, lang.t("Explorer-Kontextmenü", "Explorer context menu"), lang.t("Rechtsklick auf Ordner: „In Paneldeck öffnen“ / „Mit Claude starten“", "Right-click a folder: “Open in Paneldeck” / “Start with Claude”"), |ui| {
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
                        if let Some(e) = err {
                            self.hooks_msg = e.clone();
                            ui.colored_label(Color32::LIGHT_RED, e);
                        }
                    }
                }
                });
            });
        if ctx.input(|i| i.key_pressed(Key::Escape)) && self.capture_key.is_none() && !ctx.wants_keyboard_input() {
            open = false;
        }
        if !open || self.settings_tab != 5 {
            self.capture_key = None;
        }
        self.settings_open = open;
    }

    pub(super) fn editor_ui(&mut self, ctx: &egui::Context) {
        let lang = self.settings.language;
        let Some(ed) = self.editor.as_mut() else { return };
        let mut open = true;
        let save_key = ctx.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::S));
        egui::Window::new(lang.t("Claude-Dateien", "Claude files"))
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
                                let col = if ed.msg_ok { Color32::from_rgb(0x4c, 0xc2, 0x6a) } else { Color32::LIGHT_RED };
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
        if self.applied_font != self.settings.terminal_font {
            setup_fonts(ctx, &self.settings.terminal_font);
            self.applied_font = self.settings.terminal_font.clone();
        }
    }
}
